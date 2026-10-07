//! `acn ctl serve` (SPEC 070 CTL-1): the API on the loopback interface, a
//! worker running the registry's requests, and one summary when it stops.

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::Router;
use axum::body::Bytes;
use axum::extract::rejection::{BytesRejection, RawPathParamsRejection};
use axum::extract::{DefaultBodyLimit, RawPathParams, State};
use axum::http::HeaderMap;
use axum::routing::{MethodRouter, delete, get, post, put};
use serde::Serialize;

use crate::Refusal;
use crate::api::{self, AppState, Method, ROUTES};
use crate::registry::{Ctl, CtlConfig};

/// The one object `acn ctl serve` prints when it stops (CTL-1, CON-8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Summary {
    pub ok: bool,
    pub addr: String,
    /// API requests served.
    pub requests: u64,
    /// The runs started or adopted, ascending.
    pub run_ids: Vec<String>,
}

/// A server bound and ready to run.
pub struct Server {
    ctl: Ctl,
    listener: std::net::TcpListener,
    addr: SocketAddr,
}

/// The router: every route of the table, and nothing else (CTL-24).
pub fn router(state: AppState) -> Router {
    let mut r: Router<AppState> = Router::new();
    let mut paths: BTreeSet<&str> = BTreeSet::new();
    for route in ROUTES {
        paths.insert(route.path);
    }
    for path in paths {
        let mut mr: MethodRouter<AppState> = MethodRouter::new();
        for route in ROUTES.iter().filter(|x| x.path == path) {
            let h = move |State(s): State<AppState>,
                          p: Result<RawPathParams, RawPathParamsRejection>,
                          h: HeaderMap,
                          b: Result<Bytes, BytesRejection>| {
                api::dispatch(route, s, p, h, b)
            };
            mr = match route.method {
                Method::Get => mr.merge(get(h)),
                Method::Post => mr.merge(post(h)),
                Method::Put => mr.merge(put(h)),
                Method::Delete => mr.merge(delete(h)),
            };
        }
        r = r.route(path, mr);
    }
    r.fallback(api::not_found)
        .method_not_allowed_fallback(api::method_not_allowed)
        .layer(DefaultBodyLimit::max(1 << 20))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            api::guard,
        ))
        .with_state(state)
}

impl Server {
    /// Bind `127.0.0.1:<port>` (0: one the system picks), then open the
    /// registry: a server that cannot bind recovers nothing (CTL-1, CTL-11).
    pub fn bind(cfg: CtlConfig, port: u16) -> Result<Self, Refusal> {
        let listener = Self::listen(port)?;
        let ctl = Ctl::open(cfg)?;
        Self::with(ctl, listener)
    }

    /// Bind for a registry already open, for tests that set its hooks.
    pub fn bind_with(ctl: Ctl, port: u16) -> Result<Self, Refusal> {
        let listener = Self::listen(port)?;
        Self::with(ctl, listener)
    }

    fn listen(port: u16) -> Result<std::net::TcpListener, Refusal> {
        std::net::TcpListener::bind(("127.0.0.1", port))
            .map_err(|e| Refusal::new(500, "bind", format!("127.0.0.1:{port}: {e}")))
    }

    fn with(ctl: Ctl, listener: std::net::TcpListener) -> Result<Self, Refusal> {
        let addr = listener.local_addr().map_err(Refusal::internal)?;
        Ok(Self {
            ctl,
            listener,
            addr,
        })
    }

    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The registry the server runs, for tests.
    #[must_use]
    pub fn ctl(&self) -> Ctl {
        self.ctl.clone()
    }

    /// Serve until SIGINT, SIGTERM or `POST /v1/shutdown`, then finish the run
    /// in progress and answer the summary (CTL-1).
    pub fn run(self) -> Summary {
        let addr = self.addr.to_string();
        let fail = |ctl: &Ctl| Summary {
            ok: false,
            addr: addr.clone(),
            requests: 0,
            run_ids: ctl.run_ids(),
        };
        let Ok(rt) = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        else {
            return fail(&self.ctl);
        };
        let ctl = self.ctl.clone();
        let worker = std::thread::Builder::new()
            .name("acn-ctl-worker".into())
            .spawn({
                let c = ctl.clone();
                move || c.work()
            });
        let Ok(worker) = worker else {
            return fail(&ctl);
        };
        let requests = Arc::new(AtomicU64::new(0));
        let shutdown = Arc::new(tokio::sync::Notify::new());
        let state = AppState {
            ctl: ctl.clone(),
            port: self.addr.port(),
            shutdown: shutdown.clone(),
            requests: requests.clone(),
            staging: Arc::new(AtomicU64::new(0)),
        };
        tracing::info!(addr = %self.addr, "acn ctl listening");
        let stopping = ctl.clone();
        let served = rt.block_on(async move {
            self.listener.set_nonblocking(true)?;
            let listener = tokio::net::TcpListener::from_std(self.listener)?;
            let (stopped_tx, mut stopped_rx) = tokio::sync::watch::channel(false);
            let stop = async move {
                #[cfg(unix)]
                let term = async {
                    match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    {
                        Ok(mut s) => {
                            s.recv().await;
                        }
                        Err(_) => std::future::pending::<()>().await,
                    }
                };
                #[cfg(not(unix))]
                let term = std::future::pending::<()>();
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    () = term => {}
                    () = shutdown.notified() => {}
                }
                // From here writes are refused while connections drain.
                stopping.stop();
                let _ = stopped_tx.send(true);
            };
            let serve = axum::serve(listener, router(state)).with_graceful_shutdown(stop);
            // A client holding a connection open does not keep the server
            // up: connections get 5 s to finish once it stops (CTL-1).
            let bound = async move {
                if stopped_rx.wait_for(|s| *s).await.is_err() {
                    std::future::pending::<()>().await;
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            };
            tokio::select! {
                r = serve => r,
                () = bound => {
                    tracing::warn!("acn ctl: connections still open after 5 s, closing");
                    Ok(())
                }
            }
        });
        // Stop taking requests; the worker finishes the run in progress, and
        // queued requests stay queued (CTL-1, CTL-21).
        ctl.stop();
        let joined = worker.join().is_ok();
        Summary {
            ok: served.is_ok() && joined && !ctl.faulted(),
            addr,
            requests: requests.load(Ordering::SeqCst),
            run_ids: ctl.run_ids(),
        }
    }
}

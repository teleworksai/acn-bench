//! The live proxy (SPEC 020 §5) against a test upstream on the loopback
//! interface: framing, timing, drops as the client sees them, upstream
//! failures, late forwards, transparency, refusals and shutdown.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use acn_emu::clock::{Clock, WallClock};
use acn_emu::link::{Delay, Direction, Link, LinkSpec, OutageCause, OutageMode, Window};
use acn_emu::proxy::{ATTEMPT_HEADER, Proxy};
use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Method, Uri, header};
use axum::response::Response;
use axum::routing::any;

const MS: i64 = 1_000_000;

/// Serve `router` on a loopback port; its base URL.
async fn serve(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{addr}")
}

fn link(direction: Direction, delay_ms: i64) -> LinkSpec {
    let mut s = LinkSpec::new("p", direction);
    if delay_ms > 0 {
        s.delay = Some(Delay {
            delay_ns: delay_ms * MS,
            jitter_ns: 0,
        });
    }
    s
}

fn dropping(direction: Direction, from_ms: i64, to_ms: i64) -> LinkSpec {
    let mut s = LinkSpec::new("p", direction);
    s.outage = Some(vec![Window {
        start_ns: from_ms * MS,
        end_ns: to_ms * MS,
        mode: OutageMode::Drop,
        cause: OutageCause::Scheduled,
    }]);
    s
}

async fn proxy(up: LinkSpec, down: LinkSpec, endpoint: &str) -> (Proxy, Arc<WallClock>) {
    let clock = Arc::new(WallClock::start());
    let p = Proxy::start(
        Link::new(up, 7).unwrap(),
        Link::new(down, 7).unwrap(),
        endpoint,
        clock.clone(),
        0,
    )
    .await
    .unwrap();
    (p, clock)
}

fn client(timeout_ms: u64) -> reqwest::Client {
    // No system proxy lookup: it is slow, and the calls must reach ours.
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(timeout_ms))
        .build()
        .unwrap()
}

/// An upstream that answers `hello` to anything.
fn hello() -> Router {
    Router::new().route("/{*p}", any(|| async { "hello" }))
}

/// Cites: EMU-40, EMU-41, EMU-42, EMU-43, EMU-47
#[tokio::test]
async fn a_request_and_its_body_are_delayed_by_their_links_and_recorded() {
    let up = serve(hello()).await;
    let c = client(5_000);
    let (p, clock) = proxy(link(Direction::Up, 50), link(Direction::Down, 30), &up).await;
    let t0 = clock.now_ns();
    let r = c
        .post(format!("http://{}/v1/x", p.addr()))
        .header(ATTEMPT_HEADER, "3")
        .body("ping!")
        .send()
        .await
        .unwrap();
    assert_eq!(r.text().await.unwrap(), "hello");
    let took = clock.now_ns() - t0;
    assert!(took >= 80 * MS, "{took}");
    assert!(took < 400 * MS, "{took}");
    let recs = p.records(3);
    assert_eq!(recs.len(), 2);
    let (u, d) = (recs[0], recs[1]);
    assert_eq!((u.direction, u.bytes), (Direction::Up, 5));
    assert_eq!((d.direction, d.bytes), (Direction::Down, 5));
    // Never early (EMU-43): handed over no earlier than delivered.
    assert!(d.received_ns.unwrap() >= d.fate.outcome.unwrap());
    assert!(u.fate.outcome.unwrap() >= u.fate.send_ns + 50 * MS);
    assert!(p.records(4).is_empty());
    assert_eq!(p.fates().len(), 2);
    p.shutdown().await;
}

/// An upstream that streams `events` with the given gaps between chunks,
/// each chunk an arbitrary slice of the event bytes.
fn streaming(chunks: Vec<(u64, &'static str)>) -> Router {
    Router::new().route(
        "/{*p}",
        any(move || {
            let chunks = chunks.clone();
            async move {
                let s = futures_util::stream::unfold(chunks.into_iter(), |mut it| async move {
                    let (gap, c) = it.next()?;
                    tokio::time::sleep(Duration::from_millis(gap)).await;
                    Some((
                        Ok::<_, std::io::Error>(Bytes::from_static(c.as_bytes())),
                        it,
                    ))
                });
                Response::builder()
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from_stream(s))
                    .unwrap()
            }
        }),
    )
}

/// Cites: EMU-41, EMU-43
#[tokio::test]
async fn an_event_stream_is_carried_event_by_event_whatever_its_chunks() {
    // Chunks split and join events; `\r\n` endings; a comment block that
    // belongs to the event after it.
    let chunks = vec![
        (0, "data: a\n"),
        (10, "\ndata: b\n\ndata: "),
        (10, "c\r\n\r\n: ping\n\n"),
        (10, "data: d\n\n"),
    ];
    let whole: String = chunks.iter().map(|(_, c)| *c).collect();
    let up = serve(streaming(chunks)).await;
    let (p, _) = proxy(link(Direction::Up, 0), link(Direction::Down, 20), &up).await;
    let r = client(5_000)
        .post(format!("http://{}/v1/s", p.addr()))
        .header(ATTEMPT_HEADER, "1")
        .send()
        .await
        .unwrap();
    assert_eq!(r.text().await.unwrap(), whole, "the bytes arrive unchanged");
    let down: Vec<u64> = p
        .records(1)
        .iter()
        .filter(|r| r.direction == Direction::Down)
        .map(|r| r.bytes)
        .collect();
    // "data: a\n\n", "data: b\n\n", "data: c\r\n\r\n", ": ping\n\ndata: d\n\n".
    assert_eq!(down, vec![9, 9, 11, 17]);
    p.shutdown().await;
}

/// Cites: EMU-44
#[tokio::test]
async fn a_lost_request_or_body_leaves_the_connection_silent() {
    let up = serve(hello()).await;
    for (u, d) in [
        (dropping(Direction::Up, 0, 60_000), link(Direction::Down, 0)),
        (link(Direction::Up, 0), dropping(Direction::Down, 0, 60_000)),
    ] {
        let (p, _) = proxy(u, d, &up).await;
        let e = client(300)
            .post(format!("http://{}/v1/x", p.addr()))
            .send()
            .await
            .unwrap_err();
        assert!(e.is_timeout(), "{e}");
        p.shutdown().await;
    }
}

/// Cites: EMU-44
#[tokio::test]
async fn a_lost_event_cuts_the_response_at_the_next_delivered_one() {
    // The first event is read at once and lost (the downlink drops [0, 100 ms));
    // the second, 200 ms later, is delivered: the client sees a broken
    // response then, long before its timeout.
    let up = serve(streaming(vec![(0, "data: a\n\n"), (200, "data: b\n\n")])).await;
    let c = client(5_000);
    let (p, clock) = proxy(
        link(Direction::Up, 0),
        dropping(Direction::Down, 0, 100),
        &up,
    )
    .await;
    let t0 = clock.now_ns();
    let result = async {
        let r = c.post(format!("http://{}/v1/s", p.addr())).send().await?;
        r.text().await
    }
    .await;
    let e = result.unwrap_err();
    assert!(!e.is_timeout(), "{e}");
    assert!(clock.now_ns() - t0 < 2_000 * MS);
    p.shutdown().await;
}

/// Cites: EMU-45
#[tokio::test]
async fn an_unreachable_upstream_closes_the_connection_at_once() {
    // A port nothing listens on.
    let free = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = free.local_addr().unwrap();
    drop(free);
    let (p, _) = proxy(
        link(Direction::Up, 0),
        link(Direction::Down, 0),
        &format!("http://{addr}"),
    )
    .await;
    let e = client(5_000)
        .post(format!("http://{}/v1/x", p.addr()))
        .send()
        .await
        .unwrap_err();
    assert!(!e.is_timeout(), "{e}");
    p.shutdown().await;
}

/// Cites: EMU-45
#[tokio::test]
async fn a_delivered_request_is_forwarded_after_its_client_left() {
    let seen = Arc::new(AtomicUsize::new(0));
    let s = seen.clone();
    let up = serve(Router::new().route(
        "/{*p}",
        any(move || {
            let s = s.clone();
            async move {
                s.fetch_add(1, Ordering::SeqCst);
                "ok"
            }
        }),
    ))
    .await;
    let (p, _) = proxy(link(Direction::Up, 200), link(Direction::Down, 0), &up).await;
    let e = client(50)
        .post(format!("http://{}/v1/x", p.addr()))
        .send()
        .await
        .unwrap_err();
    assert!(e.is_timeout());
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        seen.load(Ordering::SeqCst),
        1,
        "the late request was not forwarded"
    );
    p.shutdown().await;
}

/// Cites: EMU-46
#[tokio::test]
async fn requests_are_forwarded_unchanged_but_for_host_base_path_and_hop_headers() {
    let up = serve(Router::new().route(
        "/{*p}",
        any(
            |method: Method, uri: Uri, headers: HeaderMap, body: Bytes| async move {
                let get = |k: &str| headers.get(k).map(|v| v.to_str().unwrap().to_owned());
                serde_json::json!({
                    "method": method.as_str(),
                    "uri": uri.to_string(),
                    "host": get("host"),
                    "custom": get("x-custom"),
                    "attempt": get(ATTEMPT_HEADER),
                    "keep": get("keep-alive"),
                    "body": String::from_utf8(body.to_vec()).unwrap(),
                })
                .to_string()
            },
        ),
    ))
    .await;
    let base = format!("{up}/base");
    let (p, _) = proxy(link(Direction::Up, 0), link(Direction::Down, 0), &base).await;
    let r = client(5_000)
        .put(format!("http://{}/v1/y?q=1", p.addr()))
        .header("x-custom", "kept")
        .header(ATTEMPT_HEADER, "9")
        .header("keep-alive", "timeout=5")
        .body("payload")
        .send()
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    assert_eq!(v["method"], "PUT");
    assert_eq!(v["uri"], "/base/v1/y?q=1");
    assert_eq!(v["host"], up.trim_start_matches("http://"));
    assert_eq!(v["custom"], "kept");
    assert!(v["attempt"].is_null(), "the attempt header leaked");
    assert!(v["keep"].is_null(), "a hop-by-hop header leaked");
    assert_eq!(v["body"], "payload");
    p.shutdown().await;
}

/// Cites: EMU-41, EMU-45
#[tokio::test]
async fn upgrades_and_https_are_refused() {
    let up = serve(hello()).await;
    let (p, _) = proxy(link(Direction::Up, 0), link(Direction::Down, 0), &up).await;
    let r = client(5_000)
        .get(format!("http://{}/v1/x", p.addr()))
        .header(header::CONNECTION, "upgrade")
        .header(header::UPGRADE, "websocket")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 501);
    assert!(p.fates().is_empty(), "a refusal is offered to no link");
    p.shutdown().await;
    let clock = Arc::new(WallClock::start());
    let e = Proxy::start(
        Link::new(link(Direction::Up, 0), 1).unwrap(),
        Link::new(link(Direction::Down, 0), 1).unwrap(),
        "https://api.example.com",
        clock,
        0,
    )
    .await
    .unwrap_err();
    assert_eq!(e.reason, "endpoint");
}

/// Cites: EMU-40, EMU-44
#[tokio::test]
async fn shutdown_closes_a_silent_connection() {
    let up = serve(hello()).await;
    let (p, _) = proxy(
        dropping(Direction::Up, 0, 60_000),
        link(Direction::Down, 0),
        &up,
    )
    .await;
    let addr = p.addr();
    let call = tokio::spawn(async move {
        client(10_000)
            .post(format!("http://{addr}/v1/x"))
            .send()
            .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    p.shutdown().await;
    let e = tokio::time::timeout(Duration::from_secs(3), call)
        .await
        .expect("the held connection outlived the proxy")
        .unwrap()
        .unwrap_err();
    assert!(!e.is_timeout(), "{e}");
}

/// Cites: EMU-42, EMU-1
#[tokio::test]
async fn concurrent_connections_reach_each_link_in_time_order() {
    let up = serve(hello()).await;
    let c = client(5_000);
    let (p, _) = proxy(link(Direction::Up, 5), link(Direction::Down, 5), &up).await;
    let addr = p.addr();
    let calls: Vec<_> = (0..16)
        .map(|i| {
            let c = c.clone();
            tokio::spawn(async move {
                c.post(format!("http://{addr}/v1/x"))
                    .header(ATTEMPT_HEADER, i.to_string())
                    .body(vec![b'x'; 100 + i])
                    .send()
                    .await
                    .unwrap()
                    .text()
                    .await
                    .unwrap()
            })
        })
        .collect();
    for call in calls {
        assert_eq!(call.await.unwrap(), "hello");
    }
    for dir in [Direction::Up, Direction::Down] {
        let sends: Vec<i64> = p
            .fates()
            .iter()
            .filter(|(d, _)| *d == dir)
            .map(|(_, f)| f.send_ns)
            .collect();
        assert_eq!(sends.len(), 16);
        assert!(sends.windows(2).all(|w| w[0] <= w[1]), "{dir}: {sends:?}");
    }
    // Every attempt has its request and its body.
    for i in 0..16 {
        assert_eq!(p.records(i).len(), 2, "attempt {i}");
    }
    p.shutdown().await;
}

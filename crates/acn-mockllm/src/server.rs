//! The HTTP server (MLM-5, `live`): the engine behind `POST /v1/chat/completions`
//! and `GET /v1/models`. A request's arrival is the clock's reading when it is
//! read; the engine decides the response and every token's time; the server
//! waits on the same clock until each is due. With a [`acn_emu::clock::SimClock`]
//! the waits are instant, which is how the tests compare it with the library.
//!
//! Every response says it is the mock (MLM-4), including the 404, 405 and 413 the
//! router answers itself; those get an OpenAI-shaped error body (MLM-1).

use std::sync::{Arc, Mutex, MutexGuard};

use acn_emu::clock::Clock;
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::middleware::map_response;
use axum::response::Response;
use axum::routing::{get, post};
use serde_json::json;

use crate::engine::{self, Mock, Outcome};

/// The largest request body accepted: far above any prompt a POC sends, and a
/// bound on what one request can make the server buffer.
pub const MAX_BODY_BYTES: usize = 256 * 1024 * 1024;

/// The tenant of a request without an `Authorization` header (SPEC 030 §2).
pub const ANON_TENANT: &str = "-";

/// The profile named in the marker of a response no profile answered (MLM-4).
const NO_PROFILE: &str = "-";

#[derive(Clone)]
struct AppState {
    mock: Arc<Mutex<Mock>>,
    clock: Arc<dyn Clock>,
}

/// The router over a mock and a clock.
pub fn router(mock: Mock, clock: Arc<dyn Clock>) -> Router {
    let state = AppState {
        mock: Arc::new(Mutex::new(mock)),
        clock,
    };
    Router::new()
        .route("/v1/chat/completions", post(completions))
        .route("/v1/models", get(models))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(map_response(mark))
        .with_state(state)
}

/// Mark a response the engine did not make (MLM-4), and give an error from the
/// router an OpenAI-shaped body (MLM-1).
async fn mark(mut r: Response) -> Response {
    if r.headers().contains_key("x-acn-mockllm") {
        return r;
    }
    if r.status().is_client_error() || r.status().is_server_error() {
        let message = r
            .status()
            .canonical_reason()
            .unwrap_or("request refused")
            .to_owned();
        let kind = if r.status().is_client_error() {
            "invalid_request_error"
        } else {
            "server_error"
        };
        let status = r.status();
        r = Response::new(Body::from(engine::error_body(kind, &message)));
        *r.status_mut() = status;
        r.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
    }
    if let Ok(v) = HeaderValue::from_str(&engine::marker(NO_PROFILE)) {
        r.headers_mut().insert("x-acn-mockllm", v);
    }
    r
}

fn lock(state: &AppState) -> MutexGuard<'_, Mock> {
    state
        .mock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn tenant(headers: &HeaderMap) -> String {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or(ANON_TENANT)
        .to_owned()
}

fn response(outcome: &Outcome, body: Body) -> Response {
    let mut r = Response::new(body);
    *r.status_mut() =
        StatusCode::from_u16(outcome.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    for (k, v) in &outcome.headers {
        match (HeaderName::try_from(k.as_str()), HeaderValue::from_str(v)) {
            (Ok(k), Ok(v)) => {
                r.headers_mut().insert(k, v);
            }
            _ => tracing::warn!(header = %k, "the engine produced an invalid header; dropped"),
        }
    }
    r
}

async fn completions(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    // The arrival is read under the lock, so the engine sees arrivals in order.
    let outcome = {
        let mut mock = lock(&state);
        let arrival = state.clock.now_ns();
        mock.handle(&body, &tenant(&headers), arrival)
    };
    if outcome.status != 200 || !outcome.stream {
        if outcome.status == 200 {
            state.clock.sleep_until(outcome.respond_at_ns).await;
        }
        let body = Body::from(outcome.body.clone());
        return response(&outcome, body);
    }
    // Stream each event at its time on the run's clock.
    let clock = Arc::clone(&state.clock);
    let events = outcome.chunks.clone();
    let stream =
        futures_util::stream::unfold((clock, events.into_iter()), |(clock, mut it)| async move {
            let c = it.next()?;
            clock.sleep_until(c.at_ns).await;
            let bytes = Bytes::from(format!("data: {}\n\n", c.data));
            Some((Ok::<_, std::convert::Infallible>(bytes), (clock, it)))
        });
    response(&outcome, Body::from_stream(stream))
}

async fn models(State(state): State<AppState>) -> Response {
    let (names, blake3) = {
        let mock = lock(&state);
        let p = mock.profiles();
        (
            p.profiles
                .iter()
                .map(|p| p.name.clone())
                .collect::<Vec<_>>(),
            p.blake3.clone(),
        )
    };
    let data: Vec<_> = names
        .iter()
        .map(|n| json!({ "id": n, "object": "model", "owned_by": "acn-mockllm" }))
        .collect();
    let mut r = Response::new(Body::from(
        json!({ "object": "list", "data": data, "profiles_blake3": blake3 }).to_string(),
    ));
    // The marker (`profile=-`) is added by `mark`.
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    r
}

/// Serve on `listener` until the task is dropped.
pub async fn serve(
    listener: tokio::net::TcpListener,
    mock: Mock,
    clock: Arc<dyn Clock>,
) -> Result<(), crate::MockError> {
    axum::serve(listener, router(mock, clock)).await?;
    Ok(())
}

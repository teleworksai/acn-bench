//! The HTTP server (MLM-5, `live`): the engine behind `POST /v1/chat/completions`
//! and `GET /v1/models`. A request's arrival is the clock's reading when it is
//! read; the engine decides the response and every token's time; the server
//! waits on the same clock until each is due. With a [`acn_emu::clock::SimClock`]
//! the waits are instant, which is how the tests compare it with the library.

use std::sync::{Arc, Mutex};

use acn_emu::clock::Clock;
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use serde_json::json;

use crate::engine::{Mock, Outcome};

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
        .with_state(state)
}

fn tenant(headers: &HeaderMap) -> String {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-")
        .to_owned()
}

fn response(outcome: &Outcome, body: Body) -> Response {
    let mut r = Response::new(body);
    *r.status_mut() =
        StatusCode::from_u16(outcome.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    for (k, v) in &outcome.headers {
        if let (Ok(k), Ok(v)) = (HeaderName::try_from(k.as_str()), HeaderValue::from_str(v)) {
            r.headers_mut().insert(k, v);
        }
    }
    r
}

async fn completions(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let arrival = state.clock.now_ns();
    let outcome = {
        let mut mock = match state.mock.lock() {
            Ok(m) => m,
            Err(p) => p.into_inner(),
        };
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
        let mock = match state.mock.lock() {
            Ok(m) => m,
            Err(p) => p.into_inner(),
        };
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
    r.headers_mut().insert(
        "x-acn-mockllm",
        HeaderValue::from_str(&format!("acn-mockllm/{}", env!("CARGO_PKG_VERSION")))
            .unwrap_or(HeaderValue::from_static("acn-mockllm")),
    );
    r.headers_mut()
        .insert("content-type", HeaderValue::from_static("application/json"));
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

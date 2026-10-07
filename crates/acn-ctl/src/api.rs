//! The API (SPEC 070 §3, §4): one route table, from which both the router and
//! the OpenAPI document are made (CTL-24), and the handlers it names.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::body::Bytes;
use axum::extract::rejection::{BytesRejection, RawPathParamsRejection};
use axum::extract::{RawPathParams, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::Refusal;
use crate::registry::Ctl;
use crate::request::Submit;
use crate::resolve::{self, Endpoint};

/// An HTTP method of the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
}

impl Method {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Post => "post",
            Self::Put => "put",
            Self::Delete => "delete",
        }
    }
}

/// One route (CTL-24): what the router serves and the document describes.
#[derive(Debug, Clone, Copy)]
pub struct Route {
    pub method: Method,
    /// In axum's syntax: `{name}`, or `{*name}` for the rest of the path.
    pub path: &'static str,
    pub op: &'static str,
    pub summary: &'static str,
    /// The body's content type, when the route takes one (CTL-3).
    pub body: Option<&'static str>,
    /// The statuses it answers, the success first.
    pub codes: &'static [u16],
}

const JSON: &str = "application/json";
const TOML: &str = "application/toml";

/// Every route of the API (CTL-20 to CTL-24, CTL-30, CTL-1).
pub const ROUTES: &[Route] = &[
    Route {
        method: Method::Get,
        path: "/v1/openapi.json",
        op: "openapi",
        summary: "The API's OpenAPI 3.1 document (CTL-24).",
        body: None,
        codes: &[200],
    },
    Route {
        method: Method::Post,
        path: "/v1/scenarios",
        op: "store_scenario",
        summary: "Store a scenario's TOML, named by its BLAKE3 (CTL-20).",
        body: Some(TOML),
        codes: &[200, 400, 415],
    },
    Route {
        method: Method::Get,
        path: "/v1/scenarios/{hash}",
        op: "get_scenario",
        summary: "A stored scenario (CTL-20).",
        body: None,
        codes: &[200, 400, 404, 409],
    },
    Route {
        method: Method::Post,
        path: "/v1/runs",
        op: "submit_run",
        summary: "Submit a run request (CTL-10, CTL-21).",
        body: Some(JSON),
        codes: &[202, 200, 400, 404, 409, 415, 503],
    },
    Route {
        method: Method::Get,
        path: "/v1/runs",
        op: "list_runs",
        summary: "Every request's id and state, in submission order (CTL-21).",
        body: None,
        codes: &[200],
    },
    Route {
        method: Method::Get,
        path: "/v1/runs/{request_id}",
        op: "get_run",
        summary: "A request and its status (CTL-21).",
        body: None,
        codes: &[200, 400, 404],
    },
    Route {
        method: Method::Get,
        path: "/v1/bundles/{run_id}",
        op: "get_bundle",
        summary: "A bundle, verified: its manifest and files (CTL-22).",
        body: None,
        codes: &[200, 400, 404, 409],
    },
    Route {
        method: Method::Get,
        path: "/v1/bundles/{run_id}/files/{*path}",
        op: "get_bundle_file",
        summary: "One file the manifest lists, after a hash check (CTL-23).",
        body: None,
        codes: &[200, 400, 404, 409],
    },
    Route {
        method: Method::Get,
        path: "/v1/endpoints",
        op: "list_endpoints",
        summary: "The registered endpoints (CTL-30).",
        body: None,
        codes: &[200],
    },
    Route {
        method: Method::Put,
        path: "/v1/endpoints/{name}",
        op: "put_endpoint",
        summary: "Register a named endpoint: a backend and a URL (CTL-30).",
        body: Some(JSON),
        codes: &[200, 400, 415],
    },
    Route {
        method: Method::Delete,
        path: "/v1/endpoints/{name}",
        op: "delete_endpoint",
        summary: "Remove a named endpoint (CTL-30).",
        body: Some(JSON),
        codes: &[200, 400, 404, 409, 415],
    },
    Route {
        method: Method::Post,
        path: "/v1/shutdown",
        op: "shutdown",
        summary: "Finish the run in progress and stop (CTL-1).",
        body: Some(JSON),
        codes: &[200, 400, 415],
    },
];

fn status_text(code: u16) -> &'static str {
    match code {
        200 => "OK",
        202 => "Accepted: queued",
        400 => "A malformed request or id",
        404 => "Unknown",
        409 => "A conflict",
        413 => "The body is over 1 MiB",
        415 => "Wrong content type",
        421 => "The Host is not this server's loopback address",
        500 => "An internal failure",
        503 => "Shutting down",
        _ => "Other",
    }
}

/// The OpenAPI 3.1 document of [`ROUTES`] (CTL-24).
#[must_use]
pub fn openapi() -> Value {
    let mut paths: BTreeMap<String, serde_json::Map<String, Value>> = BTreeMap::new();
    for r in ROUTES {
        // OpenAPI names a rest-of-path parameter as any other.
        let path = r.path.replace("{*", "{");
        let params: Vec<Value> = path
            .split('{')
            .skip(1)
            .filter_map(|s| s.split('}').next())
            .map(|n| json!({"name": n, "in": "path", "required": true, "schema": {"type": "string"}}))
            .collect();
        // Every route may also answer a wrong Host, a body over the limit or
        // a write while stopping, and an internal failure.
        let mut codes: Vec<u16> = r.codes.to_vec();
        codes.extend([421, 500]);
        if r.body.is_some() {
            codes.push(413);
        }
        if r.method != Method::Get && r.op != "shutdown" {
            codes.push(503);
        }
        let mut seen = std::collections::BTreeSet::new();
        codes.retain(|c| seen.insert(*c));
        let responses: serde_json::Map<String, Value> = codes
            .iter()
            .map(|c| {
                let content = if *c >= 400 {
                    json!({JSON: {"schema": schema_ref("Refusal")}})
                } else {
                    success(r.op)
                };
                (
                    c.to_string(),
                    json!({"description": status_text(*c), "content": content}),
                )
            })
            .collect();
        let mut op = json!({
            "operationId": r.op,
            "summary": r.summary,
            "responses": responses,
        });
        if !params.is_empty() {
            op["parameters"] = Value::Array(params);
        }
        if let Some(ct) = r.body {
            let schema = match r.op {
                "submit_run" => json!({"$ref": "#/components/schemas/Submit"}),
                "put_endpoint" => json!({"$ref": "#/components/schemas/Endpoint"}),
                "store_scenario" => {
                    json!({"type": "string", "description": "A SPEC 020 scenario."})
                }
                _ => schema_ref("Empty"),
            };
            // The content type is required even for an empty body (CTL-3).
            op["requestBody"] = json!({"required": true, "content": {ct: {"schema": schema}}});
        }
        paths
            .entry(path)
            .or_default()
            .insert(r.method.as_str().into(), op);
    }
    json!({
        "openapi": "3.1.0",
        "info": {"title": "acn-ctl", "version": "1", "description": "The acn-bench control plane (SPEC 070)."},
        "servers": [{"url": "http://127.0.0.1:{port}", "variables": {"port": {"default": "8080"}}}],
        "paths": paths,
        "components": {"schemas": schemas()}
    })
}

fn schema_ref(name: &str) -> Value {
    json!({"$ref": format!("#/components/schemas/{name}")})
}

/// The content of a route's success answer.
fn success(op: &str) -> Value {
    let name = match op {
        "openapi" => {
            return json!({JSON: {"schema": {"type": "object", "description": "This document."}}});
        }
        // CTL-23: the file's bytes, typed by its extension.
        "get_bundle_file" => {
            let bin = json!({"schema": {"type": "string", "format": "binary"}});
            return json!({
                "application/vnd.apache.parquet": bin,
                JSON: bin,
                "application/octet-stream": bin,
            });
        }
        "store_scenario" => "Stored",
        "get_scenario" => "Scenario",
        "submit_run" => "Submitted",
        "list_runs" => "Runs",
        "get_run" => "Run",
        "get_bundle" => "Bundle",
        "list_endpoints" => "Endpoints",
        "put_endpoint" => "Named",
        _ => "Ok",
    };
    json!({JSON: {"schema": schema_ref(name)}})
}

/// An answer object: `ok: true` and `fields`, each required.
fn answer_schema(desc: &str, fields: &[(&str, Value)]) -> Value {
    let mut props = serde_json::Map::new();
    props.insert("ok".into(), json!({"const": true}));
    let mut required = vec![json!("ok")];
    for (k, v) in fields {
        props.insert((*k).into(), v.clone());
        required.push(json!(k));
    }
    json!({"type": "object", "description": desc, "required": required, "properties": props})
}

fn schemas() -> Value {
    let hex = json!({"type": "string", "pattern": "^[0-9a-f]{64}$"});
    let state = json!({"enum": ["queued", "running", "done", "failed"]});
    let text = json!({"type": "string"});
    json!({
        "Refusal": {
            "type": "object",
            "description": "Every refusal (CTL-2).",
            "required": ["ok", "code", "error"],
            "properties": {
                "ok": {"const": false},
                "code": {"type": "string", "description": "A stable machine-readable reason."},
                "error": {"type": "string"}
            }
        },
        "Ok": answer_schema("Done.", &[]),
        "Named": answer_schema("The endpoint registered (CTL-30).", &[("name", text.clone())]),
        "Stored": answer_schema("The scenario stored (CTL-20).", &[("hash", hex.clone()), ("name", text.clone())]),
        "Scenario": answer_schema("A stored scenario (CTL-20).", &[("hash", hex.clone()), ("name", text.clone()), ("toml", text.clone())]),
        "Submitted": answer_schema("The request: 202 when new, 200 when it was already known (CTL-21).", &[("request_id", hex.clone()), ("state", state.clone())]),
        "Runs": answer_schema("Every request, in submission order (CTL-21).", &[("runs", json!({"type": "array", "items": {
            "type": "object", "required": ["request_id", "state"],
            "properties": {"request_id": hex, "state": state}
        }}))]),
        "Run": answer_schema("A request and its status (CTL-21).", &[
            ("request_id", hex.clone()),
            ("request", json!({"type": "object", "description": "The resolved request, as `request.json` holds it (CTL-11)."})),
            ("status", schema_ref("Status")),
        ]),
        "Status": {
            "type": "object",
            "description": "A request's status (CTL-21). `run_id` is set from `running` on; `bundle_digest` and `reused` when `done`; `code` and `error` when `failed`. Times are RFC 3339 UTC.",
            "required": ["seq", "state", "submitted_at"],
            "properties": {
                "seq": {"type": "integer", "minimum": 0},
                "state": state,
                "run_id": hex,
                "bundle_digest": hex,
                "reused": {"type": "boolean"},
                "code": text,
                "error": text,
                "submitted_at": text,
                "started_at": text,
                "ended_at": text
            }
        },
        "Bundle": answer_schema("A bundle, verified (CTL-22).", &[
            ("run_id", hex.clone()),
            ("bundle_digest", hex.clone()),
            ("manifest", json!({"type": "object", "description": "The bundle's manifest (SPEC 010)."})),
            ("files", json!({"type": "array", "items": {
                "type": "object", "required": ["path", "size", "hash"],
                "properties": {"path": text, "size": {"type": "integer", "minimum": 0}, "hash": hex}
            }})),
        ]),
        "Endpoints": answer_schema("The registered endpoints (CTL-30).", &[("endpoints", json!({"type": "array", "items": {
            "type": "object", "required": ["name", "backend", "url"],
            "properties": {"name": text, "backend": text, "url": text}
        }}))]),
        "Empty": {
            "type": "object",
            "additionalProperties": false,
            "description": "`{}`; an empty body is taken as `{}`."
        },
        "Endpoint": {
            "type": "object",
            "additionalProperties": false,
            "required": ["backend", "url"],
            "properties": {
                "backend": {"enum": BACKENDS},
                "url": {"type": "string", "description": "A base URL without credentials, or `acn-mock://loopback` for `mockllm` (HAR-26)."}
            }
        },
        "Submit": {
            "type": "object",
            "additionalProperties": false,
            "description": "A run request (SPEC 070 §2, CTL-10). A `harness` request names `workload`, `backend` and `model` and no `sheet`; a `generator` request names `sheet` and none of those three. Exactly one of `hypothesis` and `seed`. Paths are relative to the workspace root.",
            "required": ["kind", "mode", "arm", "replicates"],
            "properties": {
                "kind": {"enum": ["harness", "generator"]},
                "workload": {"type": "string", "description": "harness: the workload file."},
                "backend": {"enum": BACKENDS, "description": "harness."},
                "model": {"type": "string", "description": "harness: the model, or the mock profile."},
                "sheet": {"type": "string", "description": "generator: the sheet (GEN-1)."},
                "mode": {"enum": ["sim", "live", "netem"]},
                "arm": {"enum": ["treatment", "control"]},
                "replicates": {"type": "integer", "minimum": 1},
                "vary": {"type": "object", "additionalProperties": {"type": "string"}, "description": "`name: value`, once per varied parameter."},
                "hypothesis": {"type": "string", "description": "The hypothesis file; the seed is derived from it (HYP-9)."},
                "seed": {"type": "string", "pattern": "^(0|[1-9][0-9]{0,18})$", "description": "Decimal, at most 2^63 − 1."},
                "retry": {"type": "boolean", "description": "Queue a failed request again (CTL-13)."},
                "opt": {
                    "type": "object",
                    "additionalProperties": false,
                    "description": "Run options, as the CLI's flags; an absent one takes the CLI's default. At most one of `endpoint` and `endpoint_name`.",
                    "properties": {
                        "endpoint": {"type": "string", "description": "A base URL (HAR-25)."},
                        "endpoint_name": {"type": "string", "pattern": "^[a-z0-9-]{1,64}$", "description": "A registered endpoint (CTL-30)."},
                        "max_retries": {"type": "integer", "minimum": 0},
                        "retry_base_ms": {"type": "integer", "minimum": 0},
                        "request_timeout_ms": {"type": "integer", "minimum": 0},
                        "stall_threshold_ms": {"type": "number", "minimum": 0}
                    }
                },
                "scenario": {"oneOf": [
                    {"type": "object", "additionalProperties": false, "required": ["path"], "properties": {"path": {"type": "string", "description": "A scenario file in the workspace."}}},
                    {"type": "object", "additionalProperties": false, "required": ["hash"], "properties": {"hash": {"type": "string", "pattern": "^[0-9a-f]{64}$", "description": "A stored scenario (CTL-20)."}}}
                ]}
            }
        }
    })
}

/// The backends of HAR-20.
const BACKENDS: [&str; 5] = ["mockllm", "openai", "vllm", "sglang", "anthropic"];

/// The committed form of [`openapi`]: pretty, sorted, one newline.
#[must_use]
pub fn openapi_text() -> String {
    format!(
        "{}\n",
        serde_json::to_string_pretty(&openapi()).unwrap_or_default()
    )
}

/// What every handler shares.
#[derive(Clone)]
pub struct AppState {
    pub ctl: Ctl,
    pub port: u16,
    pub shutdown: Arc<tokio::sync::Notify>,
    pub requests: Arc<AtomicU64>,
    /// Numbers each scenario staging directory, so two stores of the same
    /// bytes never share one (CTL-20).
    pub staging: Arc<AtomicU64>,
}

/// A JSON answer (CTL-2).
fn answer(status: u16, v: Value) -> Response {
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (code, [(header::CONTENT_TYPE, JSON)], format!("{v}\n")).into_response()
}

fn refusal(r: &Refusal) -> Response {
    answer(
        r.status,
        json!({"ok": false, "code": r.code, "error": r.error}),
    )
}

/// Count every request, log it without its body, and refuse a `Host` that
/// is not the loopback address and port (CTL-1, CTL-3).
pub async fn guard(State(s): State<AppState>, req: Request, next: Next) -> Response {
    s.requests.fetch_add(1, Ordering::SeqCst);
    let mut hosts = req.headers().get_all(header::HOST).iter();
    let host = match (hosts.next(), hosts.next()) {
        (Some(h), None) => Some(h),
        _ => None,
    };
    let host_ok = host.and_then(|h| h.to_str().ok()).is_some_and(|h| {
        h == format!("127.0.0.1:{}", s.port) || h == format!("localhost:{}", s.port)
    });
    let (method, path) = (req.method().clone(), req.uri().path().to_owned());
    let resp = if host_ok {
        next.run(req).await
    } else {
        refusal(&Refusal::new(
            421,
            "wrong_host",
            "the Host is not this server's loopback address (CTL-3)",
        ))
    };
    tracing::info!(%method, path, status = resp.status().as_u16(), "acn ctl");
    resp
}

/// The answer to a method a path of the table does not take.
pub async fn method_not_allowed() -> Response {
    refusal(&Refusal::new(
        405,
        "method_not_allowed",
        "this path does not take that method (SPEC 070 §3)",
    ))
}

/// The answer to a path the table does not have.
pub async fn not_found() -> Response {
    refusal(&Refusal::new(
        404,
        "not_found",
        "no such route (SPEC 070 §3)",
    ))
}

fn param<'a>(p: &'a RawPathParams, name: &str) -> &'a str {
    p.iter().find(|(k, _)| *k == name).map_or("", |(_, v)| v)
}

fn hex_id(id: &str, what: &str) -> Result<(), Refusal> {
    if resolve::is_hex64(id) {
        Ok(())
    } else {
        Err(Refusal::bad(
            "bad_id",
            format!("`{id}` is not a {what} (CTL-3)"),
        ))
    }
}

/// Run `f` off the async runtime: the registry reads and hashes files.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, Refusal> + Send + 'static,
) -> Result<T, Refusal> {
    tokio::task::spawn_blocking(f).await.map_err(|e| {
        tracing::error!(error = %e, "acn ctl handler failed");
        Refusal::internal("a handler failed")
    })?
}

/// Every route's handler: the content type, then the route's own work.
pub async fn dispatch(
    route: &'static Route,
    s: AppState,
    params: Result<RawPathParams, RawPathParamsRejection>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    // axum's own refusals, in JSON as every other answer (CTL-2).
    let params = match params {
        Ok(p) => p,
        Err(e) => return refusal(&Refusal::bad("bad_id", e.body_text())),
    };
    let body = match body {
        Ok(b) => b,
        Err(e) if e.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            return refusal(&Refusal::new(
                413,
                "payload_too_large",
                "a body is at most 1 MiB (CTL-3)",
            ));
        }
        Err(e) => return refusal(&Refusal::bad("bad_request", e.body_text())),
    };
    // While stopping only reads and a repeated shutdown are served: nothing
    // new is stored, queued or removed (CTL-1, CTL-21).
    if route.method != Method::Get && route.op != "shutdown" && s.ctl.stopping() {
        return refusal(&Refusal::new(
            503,
            "shutting_down",
            "the server is stopping (CTL-21)",
        ));
    }
    if let Some(ct) = route.body {
        let given = headers
            .get(header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .map(|t| {
                t.split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_ascii_lowercase()
            });
        if given.as_deref() != Some(ct) {
            return refusal(&Refusal::new(
                415,
                "wrong_content_type",
                format!("this route takes `{ct}` (CTL-3)"),
            ));
        }
    }
    match handle(route, s, &params, body).await {
        Ok(r) => r,
        Err(r) => refusal(&r),
    }
}

async fn handle(
    route: &'static Route,
    s: AppState,
    p: &RawPathParams,
    body: Bytes,
) -> Result<Response, Refusal> {
    let ctl = s.ctl.clone();
    match route.op {
        "openapi" => Ok(answer(200, openapi())),
        "store_scenario" => {
            let seq = s.staging.fetch_add(1, Ordering::SeqCst);
            let r = blocking(move || store_scenario(&ctl, &body, seq)).await?;
            Ok(answer(200, r))
        }
        "get_scenario" => {
            let hash = param(p, "hash").to_owned();
            hex_id(&hash, "scenario hash")?;
            let r = blocking(move || {
                let file = resolve::stored_scenario(ctl.ctl_dir(), &hash)?;
                let toml = std::fs::read_to_string(&file).map_err(Refusal::internal)?;
                let name = file
                    .file_stem()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                Ok(json!({"ok": true, "hash": hash, "name": name, "toml": toml}))
            })
            .await?;
            Ok(answer(200, r))
        }
        "submit_run" => {
            let r = Submit::parse(&body)?;
            let sub = blocking(move || ctl.submit(&r)).await?;
            let code = if sub.new { 202 } else { 200 };
            Ok(answer(
                code,
                json!({"ok": true, "request_id": sub.request_id, "state": sub.state}),
            ))
        }
        "list_runs" => {
            let list = blocking(move || ctl.list()).await?;
            let runs: Vec<Value> = list
                .into_iter()
                .map(|(id, st)| json!({"request_id": id, "state": st}))
                .collect();
            Ok(answer(200, json!({"ok": true, "runs": runs})))
        }
        "get_run" => {
            let id = param(p, "request_id").to_owned();
            hex_id(&id, "request_id")?;
            let (r, st) = blocking(move || ctl.get(&id)).await?;
            Ok(answer(
                200,
                json!({"ok": true, "request_id": param(p, "request_id"), "request": r, "status": st}),
            ))
        }
        "get_bundle" => {
            let run_id = param(p, "run_id").to_owned();
            hex_id(&run_id, "run_id")?;
            let r = blocking(move || bundle(&ctl, &run_id)).await?;
            Ok(answer(200, r))
        }
        "get_bundle_file" => {
            let (run_id, path) = (param(p, "run_id").to_owned(), param(p, "path").to_owned());
            hex_id(&run_id, "run_id")?;
            let (bytes, ct) = blocking(move || bundle_file(&ctl, &run_id, &path)).await?;
            Ok((
                StatusCode::OK,
                [(header::CONTENT_TYPE, HeaderValue::from_static(ct))],
                bytes,
            )
                .into_response())
        }
        "list_endpoints" => {
            let list = blocking(move || list_endpoints(&ctl)).await?;
            Ok(answer(200, json!({"ok": true, "endpoints": list})))
        }
        "put_endpoint" => {
            let name = param(p, "name").to_owned();
            let r = blocking(move || put_endpoint(&ctl, &name, &body)).await?;
            Ok(answer(200, r))
        }
        "delete_endpoint" => {
            let name = param(p, "name").to_owned();
            empty(&body)?;
            blocking(move || ctl.delete_endpoint(&name)).await?;
            Ok(answer(200, json!({"ok": true})))
        }
        "shutdown" => {
            empty(&body)?;
            s.ctl.stop();
            s.shutdown.notify_one();
            Ok(answer(200, json!({"ok": true})))
        }
        other => Err(Refusal::internal(format!("route `{other}` has no handler"))),
    }
}

/// The body of a route that takes none: empty, or the empty object.
fn empty(body: &[u8]) -> Result<(), Refusal> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Empty {}
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(());
    }
    serde_json::from_slice::<Empty>(body)
        .map(|_| ())
        .map_err(|e| Refusal::bad("bad_request", format!("this route takes `{{}}`: {e}")))
}

/// CTL-20: a scenario stored under its hash and its own name, loaded as
/// SPEC 020 §3 loads it; a trace-driven one is refused.
fn store_scenario(ctl: &Ctl, body: &[u8], seq: u64) -> Result<Value, Refusal> {
    let text =
        std::str::from_utf8(body).map_err(|e| Refusal::bad("scenario_invalid", e.to_string()))?;
    let table: toml::Table =
        toml::from_str(text).map_err(|e| Refusal::bad("scenario_invalid", e.to_string()))?;
    let traced = table
        .get("link")
        .and_then(|l| l.as_array())
        .is_some_and(|ls| ls.iter().any(|l| l.get("trace").is_some()));
    if traced {
        return Err(Refusal::bad(
            "trace_driven",
            "a trace-driven scenario reads files beside it: name it by its workspace path (CTL-20)",
        ));
    }
    let name = table
        .get("name")
        .and_then(|n| n.as_str())
        .filter(|n| {
            !n.is_empty()
                && n.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
        .ok_or_else(|| {
            Refusal::bad(
                "scenario_invalid",
                "a scenario's `name` is its file's name (EMU-20)",
            )
        })?
        .to_owned();
    let hash = blake3::hash(body).to_hex().to_string();
    let dir = resolve::scenario_dir(ctl.ctl_dir(), &hash);
    if !dir.exists() {
        let staging = ctl
            .ctl_dir()
            .join("scenarios")
            .join(format!(".staging-{hash}-{seq}"));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging).map_err(Refusal::internal)?;
        let file = staging.join(format!("{name}.toml"));
        std::fs::write(&file, body).map_err(Refusal::internal)?;
        if let Err(e) = acn_emu::scenario::load(&file) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(Refusal::bad("scenario_invalid", e.to_string()));
        }
        if let Err(e) = std::fs::rename(&staging, &dir) {
            let _ = std::fs::remove_dir_all(&staging);
            // Stored meanwhile by another call, with the same bytes; else a
            // real failure.
            if !dir.join(format!("{name}.toml")).is_file() {
                return Err(Refusal::internal(e));
            }
        }
    }
    Ok(json!({"ok": true, "hash": hash, "name": name}))
}

/// CTL-22: a bundle, verified, with its manifest and files.
fn bundle(ctl: &Ctl, run_id: &str) -> Result<Value, Refusal> {
    let dir = ctl.runs_dir().join(run_id);
    if !dir.join(acn_trace::bundle::MANIFEST).exists() {
        return Err(Refusal::new(
            404,
            "unknown_bundle",
            format!("no bundle {run_id} (CTL-22)"),
        ));
    }
    let v = acn_trace::bundle::verify(&dir)
        .map_err(|e| Refusal::new(409, "bundle_invalid", e.to_string()))?;
    let files: Vec<Value> = v
        .manifest
        .files
        .iter()
        .map(|(path, hash)| {
            let size = std::fs::metadata(dir.join(path))
                .map(|m| m.len())
                .unwrap_or(0);
            json!({"path": path, "size": size, "hash": hash})
        })
        .collect();
    Ok(json!({
        "ok": true,
        "run_id": v.run_id.to_hex(),
        "bundle_digest": v.bundle_digest.to_hex(),
        "manifest": v.manifest,
        "files": files,
    }))
}

/// CTL-23: one file the manifest lists, read without following a link out
/// of the bundle, after a hash check.
fn bundle_file(ctl: &Ctl, run_id: &str, path: &str) -> Result<(Vec<u8>, &'static str), Refusal> {
    let unknown = || {
        Refusal::new(
            404,
            "unknown_file",
            format!("{run_id} lists no `{path}` (CTL-23)"),
        )
    };
    let dir = ctl.runs_dir().join(run_id);
    let bytes = std::fs::read(dir.join(acn_trace::bundle::MANIFEST)).map_err(|_| unknown())?;
    let manifest: acn_trace::bundle::Manifest = serde_json::from_slice(&bytes)
        .map_err(|e| Refusal::new(409, "bundle_invalid", e.to_string()))?;
    let want = manifest.files.get(path).ok_or_else(unknown)?;
    let canon_dir = std::fs::canonicalize(&dir).map_err(|_| unknown())?;
    let file = std::fs::canonicalize(dir.join(path)).map_err(|_| unknown())?;
    if file.strip_prefix(&canon_dir).ok() != Some(std::path::Path::new(path)) {
        return Err(Refusal::new(
            409,
            "bundle_invalid",
            format!("`{path}` is a link (CTL-23)"),
        ));
    }
    let data = std::fs::read(&file).map_err(|_| unknown())?;
    if blake3::hash(&data).to_hex().as_str() != want {
        return Err(Refusal::new(
            409,
            "bundle_invalid",
            format!("`{path}` does not have its manifest hash (CTL-23)"),
        ));
    }
    let ct = if path.ends_with(".parquet") {
        "application/vnd.apache.parquet"
    } else if path.ends_with(".json") {
        JSON
    } else {
        "application/octet-stream"
    };
    Ok((data, ct))
}

fn endpoints_dir(ctl: &Ctl) -> std::path::PathBuf {
    ctl.ctl_dir().join("endpoints")
}

fn list_endpoints(ctl: &Ctl) -> Result<Vec<Value>, Refusal> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(endpoints_dir(ctl)) {
        let mut names: Vec<String> = rd
            .filter_map(Result::ok)
            .filter_map(|e| {
                e.file_name()
                    .to_string_lossy()
                    .strip_suffix(".json")
                    .map(str::to_owned)
            })
            .filter(|n| resolve::is_endpoint_name(n))
            .collect();
        names.sort();
        for n in names {
            let e = resolve::endpoint(ctl.ctl_dir(), &n)?;
            out.push(json!({"name": n, "backend": e.backend, "url": e.url}));
        }
    }
    Ok(out)
}

/// CTL-30: a named endpoint, checked as `--endpoint` is; its URL is never
/// echoed in an error.
fn put_endpoint(ctl: &Ctl, name: &str, body: &[u8]) -> Result<Value, Refusal> {
    if !resolve::is_endpoint_name(name) {
        return Err(Refusal::bad(
            "bad_id",
            "an endpoint name is 1 to 64 of a-z, 0-9 and - (CTL-30)",
        ));
    }
    let e: Endpoint =
        serde_json::from_slice(body).map_err(|e| Refusal::bad("bad_request", e.to_string()))?;
    let backend = acn_harness::wire::Backend::parse(&e.backend)
        .map_err(|e| Refusal::bad("bad_request", e.to_string()))?;
    if e.url == acn_harness::served::LOOPBACK {
        if backend != acn_harness::wire::Backend::Mockllm {
            return Err(Refusal::bad(
                "bad_request",
                "acn-mock://loopback serves only the mock (HAR-26)",
            ));
        }
    } else if e.url.is_empty() {
        return Err(Refusal::bad(
            "bad_request",
            "an endpoint has a URL (CTL-30)",
        ));
    } else if let Err(err) = acn_harness::run::endpoint_url(backend, &e.url) {
        return Err(Refusal::bad("bad_request", err.to_string()));
    }
    let dir = endpoints_dir(ctl);
    std::fs::create_dir_all(&dir).map_err(Refusal::internal)?;
    let bytes = serde_json::to_vec_pretty(&e).map_err(Refusal::internal)?;
    let tmp = dir.join(format!(".{name}.json.tmp"));
    std::fs::write(&tmp, &bytes).map_err(Refusal::internal)?;
    std::fs::rename(&tmp, dir.join(format!("{name}.json"))).map_err(Refusal::internal)?;
    Ok(json!({"ok": true, "name": name}))
}

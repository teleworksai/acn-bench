# SPEC 070 — The control plane

**Status:** Draft v0.1 (October 2026; written for T14). **Inherits:** SPEC 000, 010, 020, 040, 050, 085. **Prefix:** CTL. **Crates:** `acn-ctl`, `acn-cli`, `xtask` (the OpenAPI page).
**Purpose:**
- define one local HTTP/JSON service through which a person or an agent:
  - stores scenarios;
  - starts harness and generator runs;
  - follows them;
  - fetches their bundles;
- keep a registry of every run request on disk, so that a request is idempotent, sees an edit to its inputs, and survives a restart;
- name the endpoints a run may call (external inference nodes, partner endpoints) without ever holding their credentials.

The control plane decides nothing a run records. Every run it starts is a run of SPEC 040 or SPEC 050, with the same identity (CON-29) and the same bundle as if started from the CLI.

## 0. Terms

- **Request** — the inputs of one run, as JSON (CTL-10).
- **Resolved request** — the request with every input file's BLAKE3 and the endpoint's URL, as they were when it was submitted (CTL-11). Its **request_id** is CTL-11's hash.
- **Registry** — the requests and their status under `<runs>/ctl/` (CTL-11).
- **Server** — `acn ctl serve` (CTL-1).

## 1. The server

**CTL-1** `acn ctl serve [--port <n>] [--runs-dir <dir>]` MUST:
- serve HTTP/1.1 on `127.0.0.1` only, at the port given or one the system picks;
- log its address and every API request on stderr (`tracing`);
- run until it receives SIGINT or SIGTERM, or a `POST /v1/shutdown`. It then finishes the run in progress, if any, and stops taking requests.

On stopping, it MUST print one JSON object (CON-8):
- `ok`;
- `addr`;
- `requests`, the API requests served;
- `run_ids`, the runs it started or adopted, ascending.

`ok` is false, and the exit status non-zero, only when it could not bind, or could not finish the run in progress or write its status.

`--runs-dir` (default `runs`) MUST resolve inside the workspace root (CON-28), and not inside `hypotheses/`, `specs/`, `scenarios/measured/` or `crates/` (LOOP-20). The registry is `<runs-dir>/ctl/`. A request carries no `runs_dir` of its own.

A long-running server keeps CON-8 by printing its one object when it stops, not when it starts. There is no authentication in this version (§6, question 1). That is why the server binds only the loopback interface and checks every request's origin (CTL-3).

**CTL-2** Every response MUST be one JSON object with `ok`, except a bundle file's bytes (CTL-23).
- `ok` reports whether the API call succeeded. A run that failed is still answered `ok: true` with `state: failed`.
- A refusal has `ok: false`, a `code` and an `error`, with the HTTP status that fits:
  - 400 for a malformed request;
  - 404 for an unknown id;
  - 409 for a conflict;
  - 415 for a wrong content type;
  - 421 for a wrong `Host`;
  - 503 while shutting down;
  - 500 for an internal fault.
- Unknown fields in a request body are refused (`deny_unknown_fields`).
- No response or log line carries a credential (HAR-22).

**CTL-3** Every API request MUST carry `Host: 127.0.0.1:<port>` or `Host: localhost:<port>`, so that a page of another site, rebinding a name to the loopback address, cannot reach the server. A `POST`, `PUT` or `DELETE` MUST carry `Content-Type: application/json`, except `POST /v1/scenarios` (CTL-20), which carries `application/toml`. A browser's plain form cannot send either type, so it cannot start a run or stop the server. Every id in a path MUST be 64 lowercase hex digits (a `run_id`, `request_id` or scenario hash), or an endpoint name (CTL-30), before it is joined to any path.

## 2. The registry

**CTL-10** A run request MUST carry exactly the inputs a CLI run would, as JSON:
- `kind`: `harness` (HAR-50) or `generator` (GEN-22);
- for `harness`: `workload`, `backend` and `model`;
- for `generator`: `sheet`;
- `mode`, `arm` and `replicates`;
- `vary`, an object from parameter name to its value as text (the CLI's `--vary`, typed later as the CLI types it);
- exactly one of:
  - `hypothesis`, a path;
  - `seed`, a decimal string up to 2^63 − 1 (JSON numbers above 2^53 lose precision), permitted only for hypothesis `none` (HYP-9);
- `opt`, an object of the `opt.` options of HAR-25 that have CLI flags, by their names without the prefix. An absent option takes its default. `endpoint` may be replaced by `endpoint_name`, naming a registered endpoint (CTL-30).
- `scenario`: none, `{ "path": <file> }` naming a scenario in the workspace as the CLI does, or `{ "hash": <hash> }` naming a stored one (CTL-20).

Every path is relative to the workspace root (CON-28). It MUST resolve inside it after following links, with no `..`. A run reads its inputs from anywhere in the workspace, `hypotheses/` included, and writes only its bundle under the runs directory. LOOP-20 forbids an agent to *write* the protected paths, and nothing here writes them.

**CTL-11** The registry MUST live under `<runs-dir>/ctl/`.
- **Resolution.** On submission the request is resolved: every file it names (hypothesis, workload or sheet, scenario path) is hashed with BLAKE3 (CON-27(a)), and an `endpoint_name` is resolved to its URL. The resolved request is the request, those hashes by path, and that URL.
- **Identity.** `request_id` is the hex of `blake3("acn-bench/ctl_request/v1\0" ‖ canonical JSON of the resolved request)`. The canonical JSON is keys sorted and no insignificant whitespace, in the form of HYP-15; a known-answer vector pins it (CON-27(d)). An edit to any input, or a new URL for the endpoint, is therefore a new request. A request given with an option at its default and one without it are two requests that CON-29 makes one run (CTL-13).
- **The request file.** The resolved request is written once, with `create_new`, to `<runs-dir>/ctl/requests/<request_id>/request.json`.
- **The status.** `status.json` beside it holds:
  - `seq`, the order of submission;
  - `state`: `queued`, `running`, `done` or `failed`;
  - the `run_id`, `bundle_digest` and `reused` when done, or `code` and `error` when failed;
  - the times it was submitted, started and ended, in UTC text read through `acn_emu::clock` (CON-5(b)). No time enters any hash.

  The status is replaced by writing a new file and renaming it into place, never edited in place.
- **Restarts.** A server that starts marks every `running` request `failed` with code `interrupted`, removes that run's unfinished bundle directory as HAR-23 does, and queues its `queued` requests again by `seq`.

**CTL-12** Requests MUST run one at a time, in `seq` order, on one worker of the server.
- No two runs share the machine's clock or a loopback port, and `live` runs never compete. A `sim` run's bundle depends only on its inputs either way (CON-5).
- Before a run starts, the worker checks that every input still has the hash it was resolved with. If one does not, the request fails with `input_changed`.
- A run is called on a thread of its own, away from the server's runtime, because a run builds its own runtime.
- A panic in a run fails its request with code `internal` and does not stop the server.

**CTL-13** Submitting a request MUST be idempotent.
- The same resolved request has the same `request_id`. Resubmitting it returns its status and starts nothing.
- A request whose run failed runs again only when resubmitted with `retry: true`, which is not part of its identity. Asking while it is running is a 409.
- Before running, the worker computes the run's `run_id` (CON-29). If `<runs-dir>/<run_id>/` exists, it is verified (TRC-23):
  - a bundle that verifies is adopted: `done`, with `reused: true`, whether it was made through the server or from the CLI;
  - one that does not verify fails the request with `bundle_invalid`.

  Two requests that differ only in an option at its default are therefore one bundle. For a `live` run, adoption means no new measurement: measuring again needs another runs directory, as LOOP-12's twins do.

## 3. The API (v1)

**CTL-20** Scenarios.
- `POST /v1/scenarios` takes a scenario's TOML bytes. It MUST refuse a scenario with a trace-driven link (`[link.trace]`, EMU-10). Such a scenario reads files beside it, so it is named by its workspace path instead (CTL-10).
- Any other scenario is loaded as SPEC 020 §3 loads it; an invalid one is refused with the loader's error.
- It is stored as `<runs-dir>/ctl/scenarios/<hash>/<name>.toml`, `hash` being the BLAKE3 of the bytes (CON-27(a)) and `name` its own (EMU-20). The answer is `{ok, hash, name}`. Storing the same bytes again is a no-op.
- `GET /v1/scenarios/<hash>` answers `{ok, hash, name, toml}`.

**CTL-21** Runs.
- `POST /v1/runs` takes a request (CTL-10). It MUST resolve and validate it before queuing: paths, kind, fields, a known scenario hash, a known endpoint name of the right backend. A new request is answered 202 and an existing one 200, both with `{ok, request_id, state}`.
- `GET /v1/runs/<request_id>` answers `{ok, request_id, request, status}`.
- `GET /v1/runs` lists every request's id and state, in `seq` order.
- After shutdown begins, a `POST` is answered 503 `shutting_down`. Queued requests stay queued for the next start.

**CTL-22** `GET /v1/bundles/<run_id>` MUST verify the bundle (TRC-23) and answer:
- `ok`, `run_id`, `bundle_digest`;
- the manifest;
- every file the manifest lists, with its size and hash.

A bundle that does not verify is answered `ok: false` with `code: bundle_invalid`.

**CTL-23** `GET /v1/bundles/<run_id>/files/<path>` MUST answer the bytes of one file the manifest lists, with its content type, and nothing else.
- It is opened by its canonical path, which must lie inside `<runs-dir>/<run_id>/` without following a link out of it.
- Its bytes are checked against the manifest's hash first.
- A path the manifest does not list is a 404, so no other file under the runs directory is served.
- A request cannot set `opt.keep_content`, so no bundle made through the server carries the content sidecar (TRC-42).

**CTL-24** `GET /v1/openapi.json` MUST answer the API's OpenAPI 3.1 document.
- One route table in `acn-ctl` lists every route's method, path, request and response schemas, and status codes. Both the server's router and the document are built from it, so they cannot drift.
- The document is committed as `crates/acn-ctl/openapi.json`, and an `acn-ctl` test fails when it differs from what the table makes.
- `cargo xtask docs-inventory` renders that file as `docs/generated/ctl-api.md`, reading only the JSON, so `xtask` gains no dependency on `acn-ctl`. Its `--check` mode fails when the page is stale.

## 4. Endpoints

**CTL-30** `PUT /v1/endpoints/<name>` MUST record a named endpoint, a `backend` (HAR-20) and a base `url`, in `<runs-dir>/ctl/endpoints/<name>.json`.
- `name` is 1 to 64 lowercase ASCII letters, digits and `-`.
- A URL with credentials, a query or a fragment is refused, as `--endpoint` is (HAR-22). Credentials come from the server's environment, as a CLI run's do.
- `acn-mock://loopback` (HAR-26) is permitted, for `backend = mockllm`.
- `GET /v1/endpoints` lists the endpoints.
- `DELETE /v1/endpoints/<name>` removes one, unless a queued request names it (409).
- A request names an endpoint by `endpoint_name`, and its backend must be the endpoint's.

This is how external inference nodes and partner endpoints are reached. A run on one is still a `live` run of SPEC 040, and its manifest names the endpoint's host (EMU-49).

## 5. Acceptance tests

- `crates/acn-ctl/tests/api.rs` — CTL-2, CTL-3, CTL-10, CTL-20 to CTL-24, CTL-30:
  - each route answers one JSON object, and refusals carry their codes and statuses;
  - a wrong `Host`, a wrong content type and a malformed id are each refused;
  - refused in requests: a path outside the workspace or through a link, an unknown field, a seed with a hypothesis, an unknown scenario or endpoint, and a backend mismatch;
  - a stored scenario round-trips, and a trace-driven one is refused;
  - a bundle's files are served only as the manifest lists them, after a hash check, and a bundle that does not verify is `bundle_invalid`;
  - endpoint names are checked; a URL with credentials is refused; deleting a named endpoint is refused while a queued request uses it;
  - no response carries a credential the environment holds;
  - the router serves every route of the table, and the committed OpenAPI file is the table's.
- `crates/acn-ctl/tests/registry.rs` — CTL-11 to CTL-13:
  - `request_id` matches its known-answer vector;
  - a request runs once, and a resubmission returns its status;
  - an edited input is a new request, and an edit after submission fails with `input_changed`;
  - runs happen in `seq` order, never overlapping;
  - a restart marks an unfinished run `interrupted` and re-queues the queued ones;
  - a failed run reruns only with `retry: true`;
  - two requests that differ only in a default option share one bundle, the second adopted.
- `tests/accept/ctl.rs` — CTL-1, CON-8, CON-29:
  - a harness run and a generator run started through the server have the `run_id` and bundle digest of the same runs started from the CLI;
  - `acn ctl serve` binds only loopback and, on SIGTERM or `/v1/shutdown`, finishes the run in progress and prints one object with its `run_ids`.
- `crates/xtask` — CTL-24: the generated API page is current.

## 6. Open questions

1. **Remote access.** Serving beyond loopback needs authentication and a threat model. Until then, a remote user reaches the server through an SSH tunnel.
2. **The loop through the control plane.** `acn loop run`, `twin` and `evidence verify` as API calls (LOOP-20) are a later version. They write loop reports and verdicts, so they need the same path rules as runs.
3. **Concurrency.** One worker is simple and safe. Running independent `sim` requests in parallel would be faster, but needs a rule for what may share the machine.
4. **Richer endpoints.** Health checks, capabilities, or per-endpoint run options for inference nodes come with M3.

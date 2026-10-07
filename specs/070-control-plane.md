# SPEC 070 — The control plane

**Status:** Draft v0.1 (October 2026; written for T14). **Inherits:** SPEC 000, 010, 020, 040, 050, 085. **Prefix:** CTL. **Crates:** `acn-ctl`, `acn-cli`.
**Purpose:**
- define one local HTTP/JSON service through which a person or an agent creates scenarios, starts harness and generator runs, follows them, and fetches their bundles;
- keep a registry of every run request on disk, so a request is idempotent and survives a restart;
- name the endpoints a run may call (external inference nodes, partner endpoints) without ever holding their credentials.

The control plane decides nothing a run records: every run it starts is a run of SPEC 040 or SPEC 050, with the same identity (CON-29) and the same bundle, as if started from the CLI.

## 0. Terms

- **Request** — the inputs of one run, as JSON (CTL-10). Its **request_id** is the BLAKE3 of its canonical JSON (CTL-11).
- **Registry** — the requests and their status under `runs/ctl/` (CTL-11).
- **Server** — `acn ctl serve` (CTL-1).

## 1. The server

**CTL-1** `acn ctl serve [--port <n>] [--runs-dir <dir>]` MUST:
- serve HTTP/1.1 on `127.0.0.1` only, at the port given or one the system picks;
- log its address and every request on stderr (`tracing`);
- run until it receives SIGINT or SIGTERM, or `POST /v1/shutdown`;
- then finish the run in progress, if any, and print one JSON object (CON-8): `ok`, `addr`, `requests` (the API requests served) and `runs` (the runs started).

A long-running server keeps CON-8 by printing its one object when it stops, not when it starts. There is no authentication in this version, which is why it binds only the loopback interface (§6, question 1).

**CTL-2** Every response MUST be one JSON object with `ok`, except a bundle file's bytes (CTL-23).
- A refusal has `ok: false`, a `code` and an `error`, with the HTTP status that fits: 400 for a malformed request, 404 for an unknown id, 409 for a conflict, 500 for an internal fault.
- Unknown fields in a request body are refused (`deny_unknown_fields`).
- No response or log line carries a credential (HAR-22).

## 2. The registry

**CTL-10** A run request MUST carry exactly the inputs a CLI run would, and nothing else:
- `kind`: `harness` (HAR-50) or `generator` (GEN-22);
- for `harness`: `workload`, `backend`, `model`;
- for `generator`: `sheet`;
- for both:
  - `mode`, `arm`, `replicates`, `vary`, and either `hypothesis` or `seed` (HYP-9);
  - the `opt.` options of HAR-25, or `endpoint_name` naming a registered endpoint (CTL-30) in place of `opt.endpoint`;
  - `scenario`, the hash of a stored scenario (CTL-20), or none.

Every path is relative to the workspace root (CON-28) and MUST resolve inside it, with no `..` and no symbolic link out of it. A run reads its inputs and writes only its bundle under `runs/`, as LOOP-20 requires of anything an agent can drive.

**CTL-11** The registry MUST live under `runs/ctl/`.
- **The request.** Its canonical JSON (keys sorted, no insignificant whitespace, the form of HYP-15) is written once to `runs/ctl/requests/<request_id>/request.json`, where `request_id` is the hex BLAKE3 of those bytes.
  - An `endpoint_name` is recorded as given and resolved to its URL when the run starts. The URL is written to the status, not to the request.
- **The status.** `status.json` beside it holds:
  - `state`: `queued`, `running`, `done` or `failed`;
  - the `run_id` and `bundle_digest` when done, or `code` and `error` when failed;
  - the times it was submitted, started and ended, as wall-clock UTC text.

  It is replaced as a whole, never edited in place.
- **Restarts.** A server that starts finds every `running` request it did not finish and marks it `failed` with `interrupted`. A run's bundle directory that did not finish is removed, as HAR-23 already does.

**CTL-12** Requests MUST run one at a time, in the order they were submitted, by one worker of the server. So no two runs share the machine's clock or a loopback port, and `live` runs never compete. A `sim` run's bundle is unaffected: it depends only on its inputs (CON-5).

**CTL-13** Submitting a request MUST be idempotent:
- the same request has the same `request_id`, and resubmitting it returns its status without running it again;
- a request whose run failed is run again only when resubmitted with `retry: true`, which is not part of its identity;
- the run's own identity is CON-29's, so two requests that differ only in an option at its default, which CON-29 drops, still run as one `run_id`. The second finds the bundle and is `done` without running.

## 3. The API (v1)

**CTL-20** Scenarios.
- `POST /v1/scenarios` takes a scenario's TOML bytes. It MUST load them as SPEC 020 §3 does, refusing any invalid scenario with the loader's error, and store them as `runs/ctl/scenarios/<hash>.toml`, `hash` being the BLAKE3 of the bytes (CON-27(a)). It answers `{ok, hash}`; storing the same bytes again is a no-op.
- `GET /v1/scenarios/<hash>` answers `{ok, hash, toml}`.

**CTL-21** Runs.
- `POST /v1/runs` takes a request (CTL-10). It MUST validate it (paths, kind, flags, a known scenario hash, a known endpoint name) before queuing, and answer `202` with `{ok, request_id, state}`.
- `GET /v1/runs/<request_id>` answers `{ok, request_id, request, status}`.
- `GET /v1/runs` lists every request's id and state, in submission order.

**CTL-22** `GET /v1/bundles/<run_id>` MUST verify the bundle (TRC-23) and answer:
- `ok`, `run_id`, `bundle_digest`;
- the manifest;
- every file the manifest lists, with its size and hash.

A bundle that does not verify is answered with `ok: false`, `code: bundle_invalid`.

**CTL-23** `GET /v1/bundles/<run_id>/files/<path>` MUST answer the bytes of one file the manifest lists, with its content type, and nothing else. The bytes are checked against the manifest's hash first. A path the manifest does not list is a 404, so no other file under `runs/` is served.

**CTL-24** `GET /v1/openapi.json` MUST answer the API's OpenAPI 3.1 document, made by `acn-ctl` from the same route table the server serves. `cargo xtask docs-inventory` MUST render it as `docs/generated/ctl-api.md`, and `--check` MUST fail when the page is stale.

## 4. Endpoints

**CTL-30** `PUT /v1/endpoints/<name>` MUST record a named endpoint: a `backend` (HAR-20) and a base `url`, under `runs/ctl/endpoints/<name>.json`.
- `name` is lowercase ASCII letters, digits and `-`.
- A URL with credentials, a query or a fragment is refused, as `--endpoint` is (HAR-22). Credentials come from the server's environment, as a CLI run's do.
- `GET /v1/endpoints` lists them, and `DELETE /v1/endpoints/<name>` removes one.
- A request names an endpoint by `endpoint_name`, and its backend must be the endpoint's.

This is how external inference nodes and partner endpoints are reached: a run on one is still a `live` run of SPEC 040, and its manifest names the endpoint's host (EMU-49).

## 5. Acceptance tests

- `crates/acn-ctl/tests/api.rs` — CTL-2, CTL-10, CTL-20 to CTL-23, CTL-30:
  - each endpoint answers one JSON object, and refusals carry their codes;
  - a path outside the workspace, an unknown field, an unknown scenario or endpoint, and a URL with credentials are each refused;
  - a stored scenario round-trips;
  - a bundle's files are served only as the manifest lists them, after a hash check.
- `crates/acn-ctl/tests/registry.rs` — CTL-11 to CTL-13:
  - a request runs once, and a resubmission returns its status;
  - runs happen in submission order;
  - a restart marks an unfinished run `interrupted`;
  - a failed run reruns only with `retry: true`.
- `tests/accept/ctl.rs` — CTL-1, CTL-24, CON-8, CON-29:
  - a harness run and a generator run started through the server have the run_id and bundle digest of the same runs started from the CLI;
  - `acn ctl serve` prints one object when shut down;
  - the OpenAPI page is current.

## 6. Open questions

1. **Remote access.** Serving beyond loopback needs authentication and a threat model; until then, a remote user reaches the server through an SSH tunnel.
2. **The loop through the control plane.** `acn loop run`, `twin` and `evidence verify` as API calls (LOOP-20) are a later version: they write loop reports and verdicts, so they need the same path rules as runs.
3. **Concurrency.** One worker is simple and safe. Running independent `sim` requests in parallel would be faster; it needs a rule for what may share the machine.

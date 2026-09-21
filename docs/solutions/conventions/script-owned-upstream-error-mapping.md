---
title: "Route scripts own upstream error mapping but must not absorb policy rejections"
date: 2026-09-21
category: conventions
module: upstream HTTP error mapping
problem_type: convention
component: upstream
severity: medium
applies_when:
  - "Adding or reviewing a route script that calls `ctx.http.get`"
  - "Adding or reviewing a route script that streams bytes through `ctx.http.pipe`"
  - "Deciding which failures answer the scenario's 502 error code and which stay 500 `script_error`"
  - "Writing end-to-end tests that serve the in-repo demo fixture against a fake upstream"
related_components: [script, upstream, demo]
tags: [error-mapping, upstream, script, demo-fixture, fail-closed, manifest, download, pipe, streaming, range, end-to-end-tests]
---

# Route scripts own upstream error mapping but must not absorb policy rejections

## Context

T4 fixed the engine boundary of `ctx.http.get`: a final non-redirect HTTP response, including 4xx/5xx, is data, while DNS, connection, TLS, and timeout failures throw a catchable error whose `error.code` is `upstream_unreachable` (`docs/contracts/ctx-api.md`, `src/upstream.rs`). Redirects are followed manually for at most three hops, with protocol and host re-validation on every hop (`UpstreamAccess::send_following_redirects` in `src/upstream.rs`). ADR 0005 states the split: a response means the semantics belong to the upstream, no response means they belong to the mock, and the business call belongs to the script; its T6 amendment records where `ctx.http.pipe` must deviate, because a streamed body never reaches the script (`plans/adr/0005-upstream-failure-semantics.md`).

That engine boundary is not the client-visible error table. A prior-session probe (session history, 2026-09-20 T4) shows three questions were deliberately left open: which class a manifest JSON parse failure or a missing `data` array gets, how an allowlist/URL policy rejection should surface, and how a shipped demo fixture should be exercised in tests. T5 (issue #8) answered them inside the fixture and is merged; T6 (issue #9) added the streaming download route, which is under review in PR #19 as of this writing. Both ship from `demo/` on the feature branch.

`demo/stuntdouble.toml` declares `GET /demo/documents/manifest/:group` and `GET /demo/documents/download/:document_id` with `allow_hosts = ["metadata.example.com", "files.example.com"]`. The configuration contract requires `files.root` to be an existing directory (`docs/contracts/config.md`, `src/config.rs`), which is why the fixture carries `demo/files/.gitkeep`.

Both scripts read `ctx.env.METADATA_API_URL` (defaulting to `https://metadata.example.com/demo/documents`). The manifest script answers the fixed header `文件编码,文件标题,系统代码` with rows in `code,title,system_code` order, quotes any field containing a comma, double quote, CR, or LF, doubles internal quotes, and always ends with one newline; an empty `data` array answers the header row only (`demo/scripts/manifest.js`).

## Guidance

### 1. Classify by source before choosing the client-visible answer

`ctx.http.get` keeps the ADR 0005 split: a final HTTP response, including 4xx/5xx, is data; DNS, connection, TLS, and timeout failures are catchable transport errors; an allowlist rejection stays a policy error.

`ctx.http.pipe` cannot hand a body to the script, so it needs its own classification. The host raises these catchable `error.code` values, and the route script maps the ones it owns:

| Host outcome | `error.code` | Notes |
| --- | --- | --- |
| Final non-2xx answer to `ctx.http.pipe` | `upstream_http_error` | A streamed body cannot be inspected, so the script owns the client code (ADR 0005 T6 amendment) |
| URL that does not parse, or a scheme other than http/https | `upstream_url_invalid` | Bad upstream metadata, not a transport failure |
| Redirect chain the host cannot follow (over 3 hops or an unusable `Location`) | `upstream_redirect_error` | Separate from an allowlist rejection |
| DNS, connection, TLS, or timeout failure | `upstream_unreachable` | Also the uncaught fallback for `ctx.http.get` |
| URL or host rejected by policy, such as an allowlist miss | `script_error` | Configuration fault; never dressed up as a gateway failure |

### 2. The route script defines the route's error table

Mapping adopted by the document manifest demo:

| Upstream outcome | Script action | Client-visible result |
| --- | --- | --- |
| Status outside 200–299 | `metadataBadGateway(...)` | 502 `{"error":"metadata_bad_gateway"}` |
| `JSON.parse` fails | `metadataBadGateway(...)` | 502 `{"error":"metadata_bad_gateway"}` |
| Top-level `data` missing or not an array | `metadataBadGateway(...)` | 502 `{"error":"metadata_bad_gateway"}` |
| `error.code === "upstream_unreachable"` | `metadataBadGateway(...)` | 502 `{"error":"metadata_bad_gateway"}` |
| Any other policy or configuration error | re-`throw` | 500 `script_error` when uncaught |

The first four rows are explicit script decisions (`demo/scripts/manifest.js`). The error body is fixed JSON and the diagnostic reason goes to the server log only — never an upstream body or a stack (`demo/scripts/manifest.js`, `plans/adr/0005-upstream-failure-semantics.md`). The fallback classification lives in `src/script.rs`: an uncaught transport failure is 502 `upstream_unreachable`, a script exception is 500 `script_error`.

Mapping adopted by the document download demo (T6, `demo/scripts/download.js`):

| Host outcome | Script action | Client-visible result |
| --- | --- | --- |
| Metadata outcome (unchanged from the manifest table) | `metadataBadGateway(...)` | 502 `{"error":"metadata_bad_gateway"}` |
| `document_id` not found | `sendJson(404, ...)` | 404 `{"error":"document_not_found"}` |
| `error.code === "upstream_url_invalid"` | `sendJson(502, "pdf_url_invalid")` | 502 `{"error":"pdf_url_invalid"}` |
| `upstream_unreachable` / `upstream_http_error` / `upstream_redirect_error` | `sendJson(502, "pdf_bad_gateway")` | 502 `{"error":"pdf_bad_gateway"}` |
| Any other policy or configuration error | re-`throw` | 500 `script_error` when uncaught |

`metadata_bad_gateway` is this demo's business decision, not a mandate for every route. The invariant is: the script decides the business code, and a policy rejection is never dressed up as an upstream outage.

### 3. Catch the codes the route owns; rethrow policy rejections

For `ctx.http.get`, that means transport failures only:

```js
try {
  metadata = ctx.http.get(METADATA_URL);
} catch (error) {
  // A rejected call (allowlist, URL policy) is a configuration error and
  // stays a script error; only transport failures are gateway failures.
  if (error.code !== "upstream_unreachable") {
    throw error;
  }
  metadataBadGateway("metadata upstream unreachable");
  return;
}
```

For `ctx.http.pipe`, that means the gateway classes plus the invalid-URL class, while a policy rejection still falls through:

```js
try {
  ctx.http.pipe(item.pdf_url, { headers: { "Content-Type": "application/pdf" } });
} catch (error) {
  if (error.code === "upstream_url_invalid") {
    sendJson(502, "pdf_url_invalid");
    return;
  }
  if (
    error.code !== "upstream_unreachable" &&
    error.code !== "upstream_http_error" &&
    error.code !== "upstream_redirect_error"
  ) {
    throw error;
  }
  sendJson(502, "pdf_bad_gateway");
}
```

Both patterns ship with the fixture (`demo/scripts/manifest.js`, `demo/scripts/download.js`). An unconditional `ctx.respond(502, ...)` in the catch block turns a host missing from `allow_hosts`, or a wrong scheme, into "the upstream is down" and contradicts the fail-closed boundary in ADR 0005.

### 4. The success path is part of the same contract

Error branches must not change the success shape: the manifest answers `Content-Type: text/plain; charset=utf-8`, and both a populated and an empty `data` array end with exactly one `\n`; the download answers `Content-Type: application/pdf` plus a `Content-Disposition` filename and streams the upstream bytes unchanged. Error responses answer `application/json; charset=utf-8` with a stable `error` code (`demo/scripts/manifest.js`, `demo/scripts/download.js`).

### 5. Test a shipped fixture through the external boundary

The repository has exactly one end-to-end seam: the built binary plus real HTTP (`tests/cli.rs`). The demo fixture approach:

- Copy the shipped fixture, then rewrite only two markers — `port = 3000` and `allow_hosts = ["metadata.example.com", "files.example.com"]` — asserting each marker first, so a fixture edit fails loudly instead of silently testing something else (`demo_fixture` in `tests/cli.rs`).
- Point `METADATA_API_URL` at a stdlib TCP fake upstream and assert external behavior only: status, content type, exact body bytes, error JSON.
- The fake upstream consumes one canned response per accepted connection (`Upstream` in `tests/cli.rs`), so an N-request test needs N responses — the download scenario needs one metadata answer plus one answer per PDF request, and the metadata-failure test supplies 404 then 503.
- Establish a positive control before a negative assertion about a recorded request: the test first proves the captured head contains `host:`, then asserts that no client `x-request-id` leaked (`demo_download_route_does_not_forward_client_request_id`).
- Range coverage asserts both sides of the pipe: the recorded upstream request carries `range:`, and the client response answers 206 with the upstream `Content-Range` (`demo_download_route_forwards_range_and_preserves_content_range`).
- For `ctx.http.pipe` error codes, let the script catch and answer the code itself, then assert the stable class (`ctx_http_pipe_redirect_limit_is_catchable`, `ctx_http_pipe_url_rejection_is_catchable`).

## Why This Matters

1. It keeps the ADR 0005 boundary honest. "Has a response / has no response" is the engine's split; the business error table belongs to the script, and a catch-all erases that second layer. The T6 amendment records the one deviation: `ctx.http.pipe` raises `upstream_http_error` for a final non-2xx answer, because a streamed body cannot be data the script can inspect.
2. It separates dependency failure from operator misconfiguration. An allowlist rejection means configuration must change, not that the upstream is down; reporting it as 502 sends alerts, retries, and on-call judgement the wrong way and hides a fail-closed control. A malformed `pdf_url` is upstream data instead: the route owns it and answers `pdf_url_invalid`.
3. It gives callers a stable, assertable surface: `metadata_bad_gateway`, `pdf_url_invalid`, `pdf_bad_gateway`, and `script_error` are public categories. The engine's own error bodies carry only `request_id` and `error` (`src/server.rs`), and ADR 0005 requires stacks, upstream bodies, and internal addresses never to reach the client — a script that echoes what it fetched is what would break that guarantee (`plans/adr/0005-upstream-failure-semantics.md`, `docs/contracts/ctx-api.md`).
4. It keeps tests from giving false confidence: marker asserts pin the configuration under test, the positive control makes the negative header check meaningful, and one response per connection keeps N-request scenarios on the real network path.

## When to Apply

- Any route script that calls `ctx.http.get` and turns upstream outcomes into its own client-visible errors.
- Any route script that pipes bytes through `ctx.http.pipe` and must separate `upstream_url_invalid`, `upstream_redirect_error`, `upstream_http_error`, and `upstream_unreachable` from an allowlist `script_error`.
- Any route that defines stable internal error codes and must separate an upstream outage, bad upstream data, and this service's own configuration or script errors.
- Updating an error table, README, contract prose, or runbook: a sentence saying "metadata failures answer 502" must also name the allowlist rejection and the invalid-URL class, or link to where they are stated.
- Adding end-to-end coverage for a shipped config and script fixture, especially when copying configuration, faking an upstream, or asserting on request headers.
- When pass-through of upstream status codes is intended: make that an explicit business choice, and never let it hide a policy error.

## Examples

### Anti-pattern: catch-all turns a configuration error into 502

```js
try {
  const metadata = ctx.http.get(METADATA_URL);
  // ... build CSV ...
} catch (error) {
  ctx.respond(502, { "Content-Type": "application/json" }, '{"error":"metadata_bad_gateway"}');
}
```

With a host missing from `allow_hosts`, or `METADATA_API_URL` on the wrong scheme, this still answers "upstream gateway failure". The caller sees a retryable external dependency problem while the operator never sees the configuration fault that must be fixed.

### Working pattern and its test

```js
try {
  metadata = ctx.http.get(METADATA_URL);
} catch (error) {
  if (error.code !== "upstream_unreachable") {
    throw error;
  }
  metadataBadGateway("metadata upstream unreachable");
  return;
}
```

```rust
let upstream = Upstream::start(vec![
    UpstreamResponse::new(404, b"missing"),
    UpstreamResponse::new(503, b"unavailable"),
]);
assert_metadata_bad_gateway(&manifest_response(&upstream, &[]));
assert_metadata_bad_gateway(&manifest_response(&upstream, &[]));

let head = upstream.requests()[0].to_ascii_lowercase();
assert!(head.contains("host:"), "recorded head: {head}");
assert!(!head.contains("x-request-id"), "client header leaked upstream");
```

Two manifest requests need two canned responses, and `host:` is the positive control that makes the later negative assertion about `x-request-id` meaningful (`demo_manifest_route_maps_metadata_non_2xx_to_502`, `demo_manifest_route_does_not_forward_client_request_id`).

## Related

- [ADR 0005 — upstream failure semantics](../../../plans/adr/0005-upstream-failure-semantics.md)
- [`ctx` API contract](../../contracts/ctx-api.md)
- [Demo fixture README](../../../demo/README.md)
- [T6 issue #9](https://github.com/geeknonerd/stuntdouble/issues/9), [T5 issue #8](https://github.com/geeknonerd/stuntdouble/issues/8), and [T4 issue #7](https://github.com/geeknonerd/stuntdouble/issues/7)

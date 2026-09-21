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
  - "Deciding which failures answer the scenario's 502 error code and which stay 500 `script_error`"
  - "Writing end-to-end tests that serve the in-repo demo fixture against a fake upstream"
related_components: [script, demo]
tags: [error-mapping, upstream, script, demo-fixture, fail-closed, manifest, end-to-end-tests]
---

# Route scripts own upstream error mapping but must not absorb policy rejections

## Context

T4 fixed the engine boundary of `ctx.http.get`: a final non-redirect HTTP response, including 4xx/5xx, is data, while DNS, connection, TLS, and timeout failures throw a catchable error whose `error.code` is `upstream_unreachable` (`docs/contracts/ctx-api.md`, `src/upstream.rs:39-55`, `src/script.rs:141-176`). Redirects are followed manually for at most three hops, and redirect problems are policy errors rather than data (`src/upstream.rs:119-139`, `docs/contracts/ctx-api.md:42`). ADR 0005 states the split: a response means the semantics belong to the upstream, no response means they belong to the mock, and the business call belongs to the script (`plans/adr/0005-upstream-failure-semantics.md:19-27`).

That engine boundary is not the client-visible error table. A prior-session probe (session history, 2026-09-20 T4) shows three questions were deliberately left open: which class a manifest JSON parse failure or a missing `data` array gets, how an allowlist/URL policy rejection should surface, and how a shipped demo fixture should be exercised in tests. T5 (issue #8) answers them inside the fixture; that change sits on branch `feat/manifest-route-csv` and issue #8 is still open as of this writing.

`demo/stuntdouble.toml:14-24` declares `GET /demo/documents/manifest/:group`, its script, and `allow_hosts = ["metadata.example.com"]`. The configuration contract requires `files.root` to be an existing directory (`docs/contracts/config.md:24-25`, `src/config.rs:211-221`), which is why the fixture carries `demo/files/.gitkeep`.

The script reads `ctx.env.METADATA_API_URL` (defaulting to `https://metadata.example.com/demo/documents`), answers the fixed header `文件编码,文件标题,系统代码` with rows in `code,title,system_code` order, quotes any field containing a comma, double quote, CR, or LF, doubles internal quotes, and always ends with one newline; an empty `data` array answers the header row only (`demo/scripts/manifest.js:5-22`, `demo/scripts/manifest.js:64-79`, covered by `tests/cli.rs:1257-1301`).

## Guidance

### 1. Classify by source before choosing the client-visible answer

Three sources, three different results:

- **HTTP response, including 4xx/5xx** — data. The host keeps status-as-error disabled (`src/upstream.rs:260`), serializes the response for the bridge (`src/upstream.rs:309-314`), and the script sees the frozen `{status, headers, text(), bytes()}` wrapper (`src/script.rs:361-367`).
- **Transport failure** — a catchable host error whose public code is `upstream_unreachable` (`src/script.rs:285-293`, `types/ctx-api-v1.d.ts:50-52`).
- **Policy rejection** — URL/scheme, allowlist, redirect, or bridge-argument problems are fail-closed `Error::Policy` (`src/upstream.rs:39-55`, `src/upstream.rs:119-139`, `src/upstream.rs:166-224`).

### 2. The route script defines the route's error table

Mapping adopted by the document manifest demo:

| Upstream outcome | Script action | Client-visible result |
| --- | --- | --- |
| Status outside 200–299 | `metadataBadGateway(...)` | 502 `{"error":"metadata_bad_gateway"}` |
| `JSON.parse` fails | `metadataBadGateway(...)` | 502 `{"error":"metadata_bad_gateway"}` |
| Top-level `data` missing or not an array | `metadataBadGateway(...)` | 502 `{"error":"metadata_bad_gateway"}` |
| `error.code === "upstream_unreachable"` | `metadataBadGateway(...)` | 502 `{"error":"metadata_bad_gateway"}` |
| Any other policy or configuration error | re-`throw` | 500 `script_error` when uncaught |

The first four rows are explicit script decisions (`demo/scripts/manifest.js:39-62`). The error body is fixed JSON and the diagnostic reason goes to the server log only — never an upstream body or a stack (`demo/scripts/manifest.js:24-32`, `plans/adr/0005-upstream-failure-semantics.md:43`). The fallback classification lives in `src/script.rs:141-176`: an uncaught transport failure is 502 `upstream_unreachable`, a script exception is 500 `script_error`.

`metadata_bad_gateway` is this demo's business decision, not a mandate for every route. The invariant is: the script decides the business code, and a policy rejection is never dressed up as an upstream outage.

### 3. Catch transport failures only; rethrow everything else

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

This is the pattern the fixture ships (`demo/scripts/manifest.js:35-47`). An unconditional `ctx.respond(502, ...)` in the catch block turns a host missing from `allow_hosts`, or a wrong scheme, into "the upstream is down" and contradicts the fail-closed boundary in ADR 0005 (`plans/adr/0005-upstream-failure-semantics.md:21-27`).

### 4. The success path is part of the same contract

Error branches must not change the success shape: the manifest answers `Content-Type: text/plain; charset=utf-8`, and both a populated and an empty `data` array end with exactly one `\n` (`demo/scripts/manifest.js:75-79`, `tests/cli.rs:1295-1301`). Error responses answer `application/json; charset=utf-8` with a stable `error` code (`demo/scripts/manifest.js:26-32`).

### 5. Test a shipped fixture through the external boundary

The repository has exactly one end-to-end seam: the built binary plus real HTTP (`tests/cli.rs:1-2`). T5's approach:

- Copy the shipped fixture, then rewrite only two markers — `port = 3000` and `allow_hosts = ["metadata.example.com"]` — asserting each marker first, so a fixture edit fails loudly instead of silently testing something else (`tests/cli.rs:1186-1215`).
- Point `METADATA_API_URL` at a stdlib TCP fake upstream and assert external behavior only: status, content type, exact body bytes, error JSON (`tests/cli.rs:1230-1331`).
- The fake upstream consumes one canned response per accepted connection (`tests/cli.rs:202-203`, `tests/cli.rs:224-233`), so an N-request test needs N responses — the metadata-failure test supplies 404 then 503 for its two requests (`tests/cli.rs:1303-1311`).
- Establish a positive control before a negative assertion about a recorded request: the test first proves the captured head contains `host:`, then asserts that no client `x-request-id` leaked (`tests/cli.rs:1335-1358`).

## Why This Matters

1. It keeps the ADR 0005 boundary honest. "Has a response / has no response" is the engine's split; the business error table belongs to the script, and a catch-all erases that second layer.
2. It separates dependency failure from operator misconfiguration. An allowlist or URL policy rejection means configuration must change, not that the upstream is down; reporting it as 502 sends alerts, retries, and on-call judgement the wrong way and hides a fail-closed control.
3. It gives callers a stable, assertable surface: `metadata_bad_gateway` and `script_error` are public categories. The engine's own error bodies carry only `request_id` and `error` (`src/server.rs:193-195`), and ADR 0005 requires stacks, upstream bodies, and internal addresses never to reach the client — a script that echoes what it fetched is what would break that guarantee (`plans/adr/0005-upstream-failure-semantics.md:43`, `docs/contracts/ctx-api.md:48-59`).
4. It keeps tests from giving false confidence: marker asserts pin the configuration under test, the positive control makes the negative header check meaningful, and one response per connection keeps N-request scenarios on the real network path.

## When to Apply

- Any route script that calls `ctx.http.get` and turns upstream outcomes into its own client-visible errors.
- Any route that defines stable internal error codes and must separate an upstream outage, bad upstream data, and this service's own configuration or script errors.
- Updating an error table, README, contract prose, or runbook: a sentence saying "metadata failures answer 502" must also name the allowlist/URL policy exception, or link to where it is stated.
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

Two manifest requests need two canned responses, and `host:` is the positive control that makes the later negative assertion about `x-request-id` meaningful (`tests/cli.rs:1303-1311`, `tests/cli.rs:1343-1358`).

## Related

- [ADR 0005 — upstream failure semantics](../../../plans/adr/0005-upstream-failure-semantics.md)
- [`ctx` API contract](../../contracts/ctx-api.md)
- [Demo fixture README](../../../demo/README.md)
- [T5 issue #8](https://github.com/geeknonerd/stuntdouble/issues/8) and [T4 issue #7](https://github.com/geeknonerd/stuntdouble/issues/7)

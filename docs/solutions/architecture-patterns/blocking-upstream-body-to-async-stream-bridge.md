---
title: "Bridge blocking upstream reads into an async streaming response"
date: 2026-09-21
category: architecture-patterns
module: upstream HTTP streaming bridge
problem_type: architecture_pattern
component: upstream
severity: medium
applies_when:
  - "Adding a host capability that moves bytes from a blocking upstream reader into an async response body"
  - "Streaming local files or request uploads in later slices and reusing the pipe pattern"
  - "Changing channel capacity, chunk size, or client-disconnect handling for ctx.http.pipe"
  - "Deciding when response status and headers must be finalized relative to the first body frame"
related_components: [script, server]
tags: [streaming, backpressure, axum, tokio, spawn-blocking, ureq, ctx-http-pipe, range]
---

# Bridge blocking upstream reads into an async streaming response

## Context

This knowledge-track learning records the engine pattern behind `ctx.http.pipe` (T6, issue #9; implementation tracked in PR #19). It is an architecture pattern, not a route-specific error-mapping convention: the subject is how a synchronous, blocking byte producer is connected to an asynchronous HTTP response without putting the body in the JavaScript heap.

The script host is synchronous. Boa's `Context` is evaluated inside `tokio::task::spawn_blocking`, so `ctx.http.get` and `ctx.http.pipe` can call `ureq`'s blocking client without occupying Tokio's async workers; the worker itself is not cancellable and stops at the loop-iteration limit (not-cancellable worker and loop limit: `src/script.rs:585-587`, `src/script.rs:765-776`; the synchronous-host constraint is also documented at `src/upstream.rs:4-8`). `ctx.http.pipe` adds a second producer: after the upstream response head is known, the body is read from a blocking `ureq` reader and must reach an async axum `Body`.

The current tree has no file-stream implementation yet; `ctx.file.stream` remains listed as not implemented (`docs/contracts/ctx-api.md:25-27`). Per this session's conclusion, the pipe path is therefore also the reference seam for later local-file streaming and for the inverse upload direction: a bounded channel owned by the host, not by the script heap.

The implementation has three layers:

1. The JS prelude validates the call and records only a stream marker plus the client status and headers (`src/script.rs:379-397`).
2. The native bridge calls `UpstreamAccess::pipe`, keeps the upstream `BodyStream` in a request-scoped thread-local, and returns only status/header metadata to JavaScript (`src/script.rs:496-525`).
3. The host takes the stream after evaluation, converts it into `ResponseBody::Stream`, and axum adapts it with `Body::from_stream(ReceiverStream::new(stream))` (`src/script.rs:623-631`, `src/script.rs:665-731`, `src/server.rs:194-201`).

The public contract states the observable result: the body streams straight to the client, never enters the script heap, and preserves the upstream 2xx status plus the relevant range headers (`docs/contracts/ctx-api.md:46-53`). The ADR records why this path deviates from the ordinary "an HTTP response is data" rule for `ctx.http.get` (`plans/adr/0005-upstream-failure-semantics.md:45-54`).

Prior sessions add two constraints (session history): host functions have been synchronous since the Boa runtime landed, and scripts never use `async` / `await`, so a piped call had to stay synchronous from the script's point of view even while the body moves through a background reader; and the original #9 ticket wording was internally contradictory ("preserve the upstream status code" versus "default override to 200"), which is why the status rule had to be settled explicitly before the first frame instead of being inherited from `ctx.http.get`.

## Guidance

### 1. Keep the byte stream outside the JavaScript heap

`ctx.http.pipe` is not a byte-returning API. Its JS wrapper validates `{status, headers}`, calls the native bridge, and records `{stream: true, status, headers}`; it never receives body bytes (`src/script.rs:379-397`). The native callback stores the receiver in `PIPE_STREAM` and returns only the status/header JSON (`src/script.rs:496-525`). `ResponseBody::Stream` is explicitly documented as moving frames from the upstream connection to the client without entering the JavaScript heap (`src/script.rs:108-124`).

The host-side representation is a typed receiver, not a `Vec<u8>` or a JavaScript array:

```rust
pub type BodyStream =
    tokio::sync::mpsc::Receiver<Result<Vec<u8>, std::io::Error>>;
```

`src/upstream.rs:49-50` defines that type alias, and `src/upstream.rs:55-58` uses it as the `PipeResponse.body` field type. This is the boundary that lets the script decide policy and response metadata while the response body remains host-owned.

### 2. Decide the response head before exposing the body

`UpstreamAccess::pipe` performs the ordering deliberately (`src/upstream.rs:166-212`):

1. Parse and validate the URL, including the initial allowlist check (`src/upstream.rs:167-170`).
2. Follow redirects and obtain the upstream response head (`src/upstream.rs:171-180`).
3. Reject a final non-2xx status before creating the body channel (`src/upstream.rs:181-184`).
4. Merge script headers with the upstream range metadata, then choose the client status (`src/upstream.rs:186-203`).
5. Create the bounded channel, move the blocking reader into `spawn_blocking(pump_body)`, and return `PipeResponse` (`src/upstream.rs:204-211`).

The status default is exactly the upstream 2xx status: `client_status.unwrap_or(upstream_status)` (`src/upstream.rs:203`). That is why a normal download answers 200, while a Range response answered with 206 keeps 206; the test `ctx_http_pipe_defaults_to_the_upstream_2xx_status` uses a 207 response to prove that this is a pass-through rather than a hard-coded 200 (`tests/cli.rs:1423-1435`). The contract records the same rule (`docs/contracts/ctx-api.md:49`).

The ordering is the reason a final non-2xx cannot follow the `ctx.http.get` "response is data" rule. Once the status and headers are returned to the async side, the script cannot inspect the body and then rewrite the head. The T6 amendment therefore raises a catchable `upstream_http_error` instead (`plans/adr/0005-upstream-failure-semantics.md:45-54`; `docs/contracts/ctx-api.md:51`).

### 3. Classify failures before the stream starts, and preserve the policy boundary

The engine error variants map to stable script-visible codes in `src/upstream.rs:82-92`:

| Pre-stream failure | `error.code` | Current-tree behavior |
| --- | --- | --- |
| Initial URL does not parse, or its scheme is not `http`/`https` | `upstream_url_invalid` | `pipe` maps `Url::parse` and the initial scheme validation to `Error::InvalidUrl` (`src/upstream.rs:167-170`, `src/upstream.rs:404-411`). |
| Redirect chain exceeds three hops, `Location` is not a visible-ASCII header value (`HeaderValue::to_str()` fails), `Location` cannot be joined, or a redirect target uses a non-HTTP scheme | `upstream_redirect_error` | `send_following_redirects` calls its `redirect_error` classifier for these cases (`src/upstream.rs:240-277`; the limit is `MAX_REDIRECTS = 3` at `src/upstream.rs:18-19`). |
| DNS, connection, TLS, or timeout failure | `upstream_unreachable` | `Error::Transport` is mapped at `src/upstream.rs:90-91`; `ctx_http_pipe_transport_failure_is_catchable` covers a connection failure (`tests/cli.rs:1303-1322`), while a pipe-specific timeout has no dedicated regression yet. |
| Final non-2xx upstream response | `upstream_http_error` | `pipe` rejects the status before creating the channel (`src/upstream.rs:181-184`); the uncaught case becomes a normal 500 `script_error`, while a script can catch and map it (`tests/cli.rs:1266-1301`). |
| URL or host rejected by policy, including an allowlist miss | `script_error` | `validate` always returns `Error::Policy` for a host that is not allowlisted; it does not masquerade as an upstream failure (`src/upstream.rs:401-429`). |

The allowlist boundary is intentional. A redirect target is re-validated on every hop, and an allowlist rejection remains a policy error even when the calling API would classify a bad scheme as `upstream_redirect_error` or `upstream_url_invalid` (`src/upstream.rs:240-277`, `src/upstream.rs:401-429`). The demo route maps `upstream_url_invalid`, `upstream_http_error`, `upstream_redirect_error`, and `upstream_unreachable` to its business 502s, but deliberately rethrows other errors so an allowlist/configuration fault remains `script_error` (`demo/scripts/download.js:75-101`; `tests/cli.rs:1760-1769`).

One current-tree edge case is easy to miss: a 3xx response **without** `Location` is not converted into `Error::Redirect` by `send_following_redirects`; it falls through as the final response (`src/upstream.rs:256-275`). `pipe` then treats that 3xx as non-2xx and raises `upstream_http_error`, not `upstream_redirect_error` (`src/upstream.rs:181-184`). The demo catches both classes and answers `pdf_bad_gateway`, so its 502-level test does not distinguish them (`tests/cli.rs:1793-1808`). Per this session's conclusion, do not assume the ADR phrase "`Location` unusable" includes a missing `Location` header in the current implementation; if that code distinction matters, add an explicit host branch, a regression assertion on `error.code`, and update the contract/ADR together.

The engine-level error classification is only the first half of the story. The route script owns the client-visible business error table; that separate concern is documented in `docs/solutions/conventions/script-owned-upstream-error-mapping.md` and should not be duplicated here.

### 4. Use a bounded channel as the sync/async seam

The pipe path does not buffer the upstream body. It uses:

- a channel capacity of four frames (`PIPE_CHANNEL_CAPACITY = 4`, `src/upstream.rs:25-26`);
- a maximum read buffer of 64 KiB per read (`PIPE_CHUNK_BYTES = 64 * 1024`, `src/upstream.rs:28-29`);
- `tokio::sync::mpsc::channel(PIPE_CHANNEL_CAPACITY)` in `UpstreamAccess::pipe` (`src/upstream.rs:204-206`);
- `tokio::task::spawn_blocking` for the blocking `ureq` reader (`src/upstream.rs:205-206`).

The producer loops over `std::io::Read`, sends each frame with `blocking_send`, and treats a failed send as the end of the task (`src/upstream.rs:455-480`). `blocking_send` is the backpressure point: the reader cannot run arbitrarily ahead of the HTTP consumer, so a slow client cannot make the host buffer a whole file. The same failed-send branch is the cancellation point when the response body, and therefore the receiver, is dropped (`src/upstream.rs:455-471`).

This also explains why `ctx.http.pipe` is not subject to the `ctx.http.get` body limit. `get` reads with `.limit(MAX_RESPONSE_BYTES).read_to_vec()` (`src/upstream.rs:147-152`), where the cap is 8 MiB (`src/upstream.rs:21-23`). The pipe path never calls that limit; it streams frames through the bounded channel. `ctx_http_pipe_streams_bodies_larger_than_the_get_cap` sends 8 MiB + 1 bytes and verifies the full length (`tests/cli.rs:1492-1505`). Per this session's conclusion, the correct memory model is "bounded frames plus the reader's current buffer", not "unbounded file" and not "same 8 MiB cap as `get`"; the channel and read constants are the intended bounds.

The consumer side is equally small:

```rust
fn script_body(body: ResponseBody) -> Body {
    match body {
        ResponseBody::Text(text) => Body::from(text.into_bytes()),
        ResponseBody::Bytes(bytes) => Body::from(bytes),
        ResponseBody::Stream(stream) => Body::from_stream(ReceiverStream::new(stream)),
    }
}
```

`src/server.rs:194-201` is the exact adapter. `ReceiverStream` turns the Tokio receiver into a `Stream`; `Body::from_stream` lets axum poll it as the response body.

### 5. Preserve range semantics and header ownership

The client `Range` header is captured from the request snapshot when `evaluate` creates the request-scoped `UpstreamAccess` (`src/script.rs:559-569`). `ctx.http.get` explicitly passes `None` to its fetch path, so it does not forward client ranges (`src/upstream.rs:126-134`). `ctx.http.pipe` passes `self.client_range` into the redirect-following fetch path (`src/upstream.rs:166-180`), and `fetch` adds it as the upstream `Range` header (`src/upstream.rs:281-295`).

For response headers, the script's headers are the base list. The host copies only upstream `Content-Range` and `Content-Length` when the script did not already set the same case-insensitive name (`src/upstream.rs:186-202`). It does not blindly pass every upstream header. That gives the script ownership of headers such as `Content-Type` and `Content-Disposition` while preserving the range metadata needed by the client. `ctx_http_pipe_streams_upstream_bytes_with_status_and_headers` checks the script headers and the upstream `Content-Length` (`tests/cli.rs:1198-1223`); the demo Range test checks that `Range` reaches upstream, the client status stays 206, the body is partial, and `Content-Range` reaches the client (`tests/cli.rs:1732-1756`).

`Content-Length` is preserved, not synthesized: if the upstream uses chunked transfer and provides no `Content-Length`, the pipe path has no length to add. The demo README states that the client then receives a chunked response and that a mid-body failure can only truncate it (`demo/README.md:64-70`).

### 6. Treat post-stream failures as body termination, not as a new status

After `pipe` returns, the HTTP head is already fixed. If `pump_body` gets a read error, it sends `Err(error)` as the next channel item and stops (`src/upstream.rs:473-477`). `Body::from_stream` exposes that item as a body error; it cannot retroactively change the status or headers. The ADR and public contract state the consequence directly: once streaming starts, a mid-body upstream failure can only truncate the client body (`plans/adr/0005-upstream-failure-semantics.md:54-56`; `docs/contracts/ctx-api.md:53`).

This is why the error taxonomy has a hard split:

- a failure **before** the channel is created can become a catchable `error.code` and be mapped by the script;
- a failure **after** `pipe` has returned and the response has entered the streaming path can only terminate the body stream.

Do not try to solve a mid-body failure by buffering the whole response just to obtain a second chance at status selection. That would recreate the memory and latency problem the pipe path exists to avoid. If the route needs to inspect the body, it must use `ctx.http.get` and accept its metadata-scale 8 MiB limit (`docs/contracts/ctx-api.md:38-45`).

### 7. Keep ownership request-scoped and cleanup automatic

`HTTP_HOST` and `PIPE_STREAM` are thread-locals, not global request state (`src/script.rs:437-445`). They are initialized at the start of `evaluate` and the stream is taken out after evaluation (`src/script.rs:559-574`, `src/script.rs:623-631`). If the script throws, the stream is still taken and then dropped by the error path; `pump_body` observes the receiver loss through `blocking_send` and exits. The intended lifecycle also covers a client disconnect: when the async response body drops the receiver, the blocking producer's next send fails, and the blocking task ends (`src/upstream.rs:455-471`).

`script::execute` documents that `spawn_blocking` cannot be cancelled and that the outer deadline returns an outcome while the blocking worker stops at the loop-iteration limit (`src/script.rs:765-771`). The stream design therefore does not rely on aborting the producer task; it relies on receiver drop. Per this session's conclusion, the current test suite does not contain a dedicated client-disconnect-mid-body regression, so this lifecycle should be preserved as an invariant and given a focused test when the streaming seam is next changed. The source-level contract is in the comment and send-failure branch, but that is not a substitute for an end-to-end disconnect test.

## Why This Matters

1. **It makes binary transfer possible without a script-heap copy.** `ctx.http.get` returns `text()`/`bytes()` and is capped at 8 MiB (`docs/contracts/ctx-api.md:38-45`); the pipe path keeps bytes in a bounded host channel and is tested above that cap (`tests/cli.rs:1492-1505`). Without this seam, every large or binary response would either fail or force an explicit buffering policy into the script runtime.

2. **It makes HTTP head/body ordering explicit.** The status and headers are selected only after the upstream head is known and before the body reader is exposed (`src/upstream.rs:181-211`). This is what makes the `upstream_http_error` deviation necessary and predictable instead of an accidental inconsistency with `ctx.http.get` (ADR 0005 T6 amendment, `plans/adr/0005-upstream-failure-semantics.md:45-54`).

3. **It keeps policy failures distinguishable from upstream failures.** An allowlist rejection remains `script_error`, while malformed URLs, unfollowable redirects, transport failures, and final upstream statuses have their own catchable codes (`src/upstream.rs:82-92`, `src/upstream.rs:401-429`). A route can therefore answer a business 502 without hiding an operator configuration fault.

4. **It provides backpressure and a cleanup signal without a second execution model.** The bounded channel limits read-ahead, and receiver drop is both the client-disconnect signal and the producer-exit signal (`src/upstream.rs:455-471`). The script remains synchronous; the HTTP layer remains async; the only shared object is a host-owned stream.

## When to Apply

- When adding or reviewing `ctx.http.pipe` behavior: status selection, redirect handling, allowlist behavior, response headers, and Range semantics all cross the same pre-stream/post-stream boundary (`src/upstream.rs:166-211`; `docs/contracts/ctx-api.md:46-53`).
- When implementing a later capability that streams bytes from a blocking source into an axum response. Per this session's conclusion, `ctx.file.stream` is the natural next user of the same `spawn_blocking` + bounded `mpsc` + `Body::from_stream` seam, but its error taxonomy and head decisions must be specified for the file capability rather than copied blindly from HTTP upstream semantics (`docs/contracts/ctx-api.md:25-27`).
- When implementing the inverse upload direction: apply the same bounded-channel and backpressure principles, but keep the ownership and failure mapping specific to the upload contract.
- When a route must inspect, transform, or fully validate a body before responding: use `ctx.http.get`, not `pipe`, and account for the 8 MiB cap (`docs/contracts/ctx-api.md:43-45`).
- When changing redirect policy, error codes, Range forwarding, or `Content-Length`/`Content-Range` handling: update the ADR/contract, the demo script, and the end-to-end tests in the same change. The missing-`Location` nuance above is a concrete example of why client-level 502 assertions alone are insufficient.
- When changing the lifecycle: add or update checks for final non-2xx catchability, redirect limits, invalid URLs, bodies over 8 MiB, Range/206 preservation, and client disconnect/backpressure.

## Examples

### Canonical producer/consumer handoff

The following Rust sketch mirrors the current implementation; it is not a second execution path:

```rust
// After the upstream head has been accepted and the client status/headers
// have been decided.
let (sender, body) = tokio::sync::mpsc::channel(PIPE_CHANNEL_CAPACITY);
let reader = response.into_body().into_reader();
tokio::task::spawn_blocking(move || pump_body(reader, sender));

Ok(PipeResponse {
    status,
    headers,
    body,
})
```

The real code is `src/upstream.rs:203-211`. The producer uses `blocking_send`, not `send`, because it runs on a blocking worker and must apply backpressure synchronously without using an async-context send:

```rust
fn pump_body(
    mut reader: impl std::io::Read,
    sender: tokio::sync::mpsc::Sender<Result<Vec<u8>, std::io::Error>>,
) {
    let mut buffer = vec![0_u8; PIPE_CHUNK_BYTES];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                if sender.blocking_send(Ok(buffer[..read].to_vec())).is_err() {
                    break;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => {
                let _ = sender.blocking_send(Err(error));
                break;
            }
        }
    }
}
```

This is the current implementation at `src/upstream.rs:455-480`; the `Interrupted` arm is deliberate because a blocking read may be interrupted without ending the body.

### Async adapter

```rust
ResponseBody::Stream(stream) => Body::from_stream(ReceiverStream::new(stream)),
```

This is `src/server.rs:200`. The adapter is the only place where the host stream becomes an axum `Body`; each frame crosses the channel as a `Vec<u8>` and axum converts it to `Bytes` at the body boundary, so frames are never merged into one buffer and never handed to JavaScript.

### Route script: catch only the classes the route owns

```js
try {
  ctx.http.pipe(item.pdf_url, {
    headers: {
      "Content-Type": "application/pdf",
      "Content-Disposition": 'attachment;filename="' + String(item.code) + '.pdf"'
    }
  });
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

This mirrors `demo/scripts/download.js:75-101`. The `throw error` is the policy boundary: an allowlist rejection is not turned into a gateway failure. The full route-level mapping convention lives in `docs/solutions/conventions/script-owned-upstream-error-mapping.md`.

The verification seam is itself a durable decision (session history): the earlier spec session fixed end-to-end checks to the built binary plus real HTTP with a stdlib fake upstream, so streamed-body invariants are asserted through the external boundary rather than private bridge internals.

### Verification matrix used by the current tree

| Invariant | Test / evidence |
| --- | --- |
| Script status/headers, upstream bytes, and upstream `Content-Length` travel with the stream | `ctx_http_pipe_streams_upstream_bytes_with_status_and_headers` (`tests/cli.rs:1198-1223`) |
| Final non-2xx is catchable as `upstream_http_error`, uncaught is 500 `script_error` | `tests/cli.rs:1266-1301` |
| Transport failure is catchable as `upstream_unreachable` | `tests/cli.rs:1303-1323` |
| Default status is the upstream 2xx status, not a hard-coded 200 | `ctx_http_pipe_defaults_to_the_upstream_2xx_status` (`tests/cli.rs:1423-1435`) |
| Invalid initial URL is catchable as `upstream_url_invalid` | `tests/cli.rs:1438-1459` |
| More than three redirects is catchable as `upstream_redirect_error` | `tests/cli.rs:1462-1489` |
| Pipe streams bodies larger than the 8 MiB `get` cap | `tests/cli.rs:1492-1505` |
| Range is forwarded and 206/`Content-Range` survive | `demo_download_route_forwards_range_and_preserves_content_range` (`tests/cli.rs:1732-1756`) |
| Allowlist rejection remains a client-visible `script_error` in the demo | `tests/cli.rs:1760-1769` |
| Missing `Location` currently becomes a client-visible 502 through the final-status path | `tests/cli.rs:1793-1808`; per this session's conclusion, it does not assert `error.code` and therefore does not distinguish `upstream_http_error` from `upstream_redirect_error` |

## Related

- `plans/adr/0005-upstream-failure-semantics.md` — the T6 amendment records the streaming deviation and the pre-stream/post-stream failure split.
- `docs/contracts/ctx-api.md` — the public `ctx.http.pipe` contract, including status defaults, error codes, Range forwarding, and mid-body truncation.
- `docs/solutions/conventions/script-owned-upstream-error-mapping.md` — route-level business error mapping; this document deliberately leaves that table to it.
- `tests/cli.rs` — the end-to-end fake-upstream coverage for the transport, redirect, url, status, size, and Range invariants.
- PR #19 — implementation and validation context for the T6 pattern; the current tree behavior above is the source of truth even while the PR is under review.
- Related issues: #9 (T6 source), #7 (allowlist and transport boundary), #8 (route-level error mapping), #3 (parent spec; file streaming and uploads are the later reuse scope).

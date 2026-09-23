# `ctx` host API contract

**English** \| [中文](./ctx-api.zh-CN.md)

- **Status**: public and frozen for the v1 slice; this page documents the implemented subset (T1–T11)
- **Applies to**: `apiVersion` 1
- **Stability**: additive within an `apiVersion`; removals require a new `apiVersion`

## Versioning

Every script context exposes `ctx.apiVersion`. The first value is `"1"`.

Within one `apiVersion`:

- host functions may be added;
- existing function names, argument order, and return shapes must not break;
- removing or renaming a function requires a new `apiVersion`;
- the product may support more than one `apiVersion` at a time.

## Feature set

| Group | API | Status |
| --- | --- | --- |
| Request snapshot | `ctx.request.method` / `path` / `params` / `query` / `headers` / `bodyText` | implemented (T2) |
| Request snapshot | `ctx.request.bodyBytes` | pending (not in this slice) |
| Upstream HTTP | `ctx.http.get` | implemented (T4) |
| Upstream HTTP | `ctx.http.get` `opts.retries` / `backoff` | pending |
| Upstream HTTP | `ctx.http.request` | pending |
| Binary passthrough | `ctx.http.pipe` | implemented (T6) |
| Files | `ctx.file.readText` / `ctx.file.readBytes` / `ctx.file.stream` | implemented (T11) |
| Response | `ctx.respond` | implemented (T2) |
| Request-local state | `ctx.local` | pending |
| Logging | `ctx.log.info` / `warn` / `error` | implemented (T2) |
| Environment | `ctx.env` | implemented (T2) |
| Timers | `setTimeout` / `setInterval` | pending |

## Implemented subset

- `ctx.apiVersion` is `"1"`.
- `ctx.request` is a read-only snapshot with `method`, `path`, `params`, `query`, `headers`, and `bodyText`. Header names are lowercased; `bodyText` is `null` when the request body is not valid UTF-8. Query names and values are percent-decoded. A repeated query or header name keeps the last value in the snapshot.
- `ctx.respond(status, headers, body)` accepts a status in `[100, 599]`, headers as an object or `[name, value]` pairs, and a body that is a string, byte array, `Uint8Array`, `ArrayBuffer`, or a `ctx.file.stream` handle. The first call wins and returns `true`; later calls are ignored, return `false`, and produce a server-side warning. A byte array must contain integers in `[0, 255]`. Header names and values are validated when the call is made; malformed pairs raise a catchable `script_error` before a Response is recorded and are never silently dropped.
- `ctx.http.get(url, opts)` performs an allowlisted upstream GET and returns `{status, headers, text(), bytes()}`.
  - `url` must be an absolute `http` or `https` URL whose host matches `upstream.allow_hosts` case-insensitively; the port is not part of the match, and IP literals and `localhost` require an explicit entry.
  - `opts` must be a plain object and accepts only `{ timeout_ms }`. Unknown string or symbol keys, inherited keys, non-object values, and explicit `null`, `NaN`, `Infinity`, non-integer, or non-positive `timeout_ms` values are script errors (fail-closed).
  - The timeout defaults to `upstream.timeout_ms` (15000) and `opts.timeout_ms` overrides it per call. `opts.timeout_ms` is an upper bound: the effective upstream timeout is capped by the remaining script budget, with a small reply margin reserved so a timeout can surface as an upstream failure.
  - Redirects are followed manually, at most 3 hops; protocol and host allowlist are re-validated before every hop.
  - `status` and `headers` are snapshots; header names are lowercased and a repeated name keeps the last value. `text()` decodes UTF-8 lossily; `bytes()` returns a `Uint8Array`. Response bodies are capped at 8 MiB per call; larger or binary payloads belong to `ctx.http.pipe`.
  - Upstream requests are direct. Environment proxy variables (`HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, and lowercase variants) are not used.
  - Upstream 4xx/5xx responses are data and are never thrown. DNS, connection, TLS, and timeout failures throw a catchable error whose `error.code` is `"upstream_unreachable"`; an invalid URL, unsupported scheme, or allowlist rejection is a `"script_error"`. If uncaught, a transport failure returns 502 `upstream_unreachable` with a `request_id`.
- `ctx.http.pipe(url, opts)` streams one allowlisted upstream GET body straight to the client Response; the bytes never enter the script heap.
  - `url` follows the same absolute-URL and allowlist rules as `ctx.http.get`. Redirects are followed manually, at most 3 hops, with protocol and host re-validation on every hop.
  - `opts` is optional and accepts only `{status, headers}`. `status` must be an integer in `[100, 599]`; `headers` accepts the same object or `[name, value]` pair shapes as `ctx.respond`. Unknown keys, non-plain objects, and malformed values are script errors (fail-closed). Header pairs are validated before the upstream call; a malformed pair raises a catchable `script_error` and no upstream request is made.
  - `status` defaults to the upstream 2xx status, so a plain download answers `200` and a Range request answered with `206` keeps its partial-response status. Script-supplied headers are sent as given; upstream `Content-Range` and `Content-Length` are preserved unless the script sets a header with the same name.
  - The client's `Range` request header is forwarded to the upstream call.
  - Upstream responses in `[200, 299]` start the stream. A final non-2xx response raises a catchable error whose `error.code` is `"upstream_http_error"`; a URL that does not parse or whose scheme is not `http`/`https` raises `"upstream_url_invalid"`; a redirect chain the host cannot follow (more than 3 hops or an unusable `Location`) raises `"upstream_redirect_error"`; DNS, connection, TLS, and timeout failures raise `"upstream_unreachable"`; an allowlist rejection raises `"script_error"`.
  - The first `ctx.respond` or `ctx.http.pipe` call wins; a later call is ignored, returns `false`, and produces a server-side warning. An uncaught `upstream_http_error` is an ordinary script error (500 `script_error`), never `502 upstream_unreachable`.
  - The upstream body read stays bounded by the effective upstream timeout, and a body already being streamed cannot be transformed or converted into a buffered Response; scripts that need the bytes use `ctx.http.get`. Once the stream starts, a mid-body upstream failure can only truncate the client body, because the status and headers are already on the wire. The host records that failure in the request log as `upstream_stream_error`; see the [CLI contract](cli.md) for the log fields.
- `ctx.file.readText(path)` and `ctx.file.readBytes(path)` read one file inside the configured static file root. A path is a filesystem path relative to `[files] root` and is never parsed as a URL or a platform-specific path on another platform; absolute paths and any `..` component are rejected before resolution, and symlinks are followed only while the canonicalized target stays inside the canonicalized root. `readText` decodes strict UTF-8; `readBytes` returns a `Uint8Array`. Both are capped at 8 MiB and throw catchable errors:
  - `file_path_invalid` — absolute path, `..` component, empty path, or a symlink that escapes the root;
  - `file_not_found` — no file at the resolved path;
  - `file_too_large` — the file exceeds the 8 MiB buffered cap;
  - `file_encoding_error` — `readText` found bytes that are not valid UTF-8;
  - `file_io_error` — the path is not a regular file or another I/O failure occurred.
  An uncaught file error is an ordinary script error (500 `script_error`); the host never maps a missing file to 404 automatically.
- `ctx.file.stream(path)` opens one file inside the root and returns an opaque, single-consumption handle with no readable properties. The handle is valid only as the `body` argument of `ctx.respond`, and its open-time failures use the same catchable codes as the buffered reads.
- `ctx.respond(200, headers, ctx.file.stream(path))` streams the file to the client without entering the script heap. The script status must be `200`; the host owns `Accept-Ranges`, `Content-Length`, and `Content-Range`, and a script-supplied value for those headers — or a non-200 status with a file stream — raises a catchable `script_error`.
  - A valid single range (`bytes=N-M`, `bytes=N-`, or `bytes=-N`) answers `206` with a clamped end offset and the correct `Content-Length` and `Content-Range`. An unusable Range header — unsatisfiable, malformed, multi-range, or any range on an empty file — answers `416` with `Content-Range: bytes */<size>` and no body. A present `If-Range` disables Range handling and answers a full `200`.
  - `Content-Type` is never inferred; the script sets it. HEAD is not auto-mapped to a GET Route; only an explicitly declared HEAD Route matches. Buffered `ctx.respond` bodies keep the existing no-Range behavior.
  - Once the stream starts, a mid-body file read failure truncates the client body, because the status and headers are already on the wire, and the host records `file_stream_error` in the request log. File paths never reach the client or the logs. See the [CLI contract](cli.md) for the log fields.
- `ctx.env` is the process environment snapshot. No `.env` file is loaded.
- `ctx.log.info` / `warn` / `error` write to server logs only and never to the client Response. Messages are not redacted or filtered: the script author must keep request/Response bodies, tokens, cookies, and other secrets out of them. The host itself never logs bodies automatically.
- A script that throws, exceeds `sandbox.script_timeout_ms` (default 10000), or fails to load returns 500 `script_error`; a script that finishes without producing a Response returns 500 `script_no_response`; an uncaught upstream transport failure returns 502 `upstream_unreachable`. All three carry a `request_id`.

## Pending capabilities

The following capabilities are part of the longer v1 plan but are not implemented yet. They are intentionally absent from `ctx` and from `types/ctx-api-v1.d.ts`; calling an absent member is an ordinary script error and maps to 500 `script_error`.

- `ctx.request.bodyBytes`
- `ctx.http.request` and `ctx.http.get` retry/backoff options
- `ctx.local`
- `setTimeout` and `setInterval`

## Security boundary

Scripts receive no raw `fetch`, `fs`, `os`, `subprocess`, or `socket`. All external capabilities come from host functions and are subject to:

- script deadline (`sandbox.script_timeout_ms`) that answers the client with 500 `script_error`
- network allowlist
- static file root confinement for `ctx.file` (upload size limits land with T12)
- stack traces never returned to clients

Boa 0.22 exposes no heap metric, heap limit, or interrupt hook, so no heap cap is enforced; a script abandoned at the deadline is stopped only by a host-side loop-iteration backstop. The T3 amendment to [ADR 0003](../../plans/adr/0003-script-first-multi-runtime.md) records that tradeoff, the remaining resource bounds, and the process-isolation upgrade path.

## Type definitions

The product must publish the `.d.ts` file for each supported `apiVersion`; [ADR 0012](../../plans/adr/0012-release-artifacts-and-supply-chain.md) makes it part of the release artifact set. T9 (#12) wires that pipeline. The source definition is already part of this contract.

The `apiVersion` 1 source definition for the implemented first-slice subset is [`types/ctx-api-v1.d.ts`](../../types/ctx-api-v1.d.ts). Additions within `apiVersion` 1 must update that file and the contract in the same change.

## Deprecation

- Warn for at least one minor version before removal.
- A removed function requires a new `apiVersion`.
- Migration notes go into `CHANGELOG.md` and release notes.

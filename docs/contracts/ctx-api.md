# `ctx` host API contract

**English** \| [中文](./ctx-api.zh-CN.md)

- Status: draft; slices T2–T6 implement the `apiVersion`, `request`, `http.get`, `http.pipe`, `respond`, `log`, and `env` subset
- Applies to: `apiVersion` 1
- Stability: additive within an `apiVersion`; removals require a new `apiVersion`

## Versioning

Every script context exposes `ctx.apiVersion`. The first value is `"1"`.

Within one `apiVersion`:

- Host functions may be added.
- Existing function names, argument order, and return shapes must not break.
- Removing or renaming a function requires a new `apiVersion`.
- The product may support more than one `apiVersion` at a time.

## Capability groups

| Group | API | Status |
| --- | --- | --- |
| Request | `ctx.request` | implemented (T2) |
| Upstream HTTP | `ctx.http.get` | implemented (T4) |
| Upstream HTTP | `ctx.http.request` | not implemented |
| Binary passthrough | `ctx.http.pipe` | implemented (T6) |
| Files | `ctx.file.readText` / `ctx.file.readBytes` / `ctx.file.stream` | not implemented |
| Response | `ctx.respond` | implemented (T2) |
| Request-local state | `ctx.local` | not implemented |
| Logging | `ctx.log.info` / `warn` / `error` | implemented (T2) |
| Environment | `ctx.env` | implemented (T2) |
| Timers | `setTimeout` / `setInterval` | not implemented |

## Implemented subset (slices T2–T6)

- `ctx.apiVersion` is `"1"`.
- `ctx.request` is a read-only snapshot with `method`, `path`, `params`, `query`, `headers`, and `bodyText`. Header names are lowercased; `bodyText` is `null` when the request body is not valid UTF-8.
- `ctx.respond(status, headers, body)` accepts a status in `[100, 599]`, headers as an object or `[name, value]` pairs, and a body that is a string, byte array, or `Uint8Array`. The first call wins; later calls are ignored and produce a server-side warning.
- `ctx.http.get(url, opts)` performs an allowlisted upstream GET and returns `{status, headers, text(), bytes()}`.
  - `url` must be an absolute `http` or `https` URL whose host matches `upstream.allow_hosts` case-insensitively; the port is not part of the match, and IP literals and `localhost` require an explicit entry.
  - `opts` must be a plain object and accepts only `{ timeout_ms }`. Unknown string or symbol keys, inherited keys, non-object values, and explicit `null`, `NaN`, `Infinity`, non-integer, or non-positive `timeout_ms` values are script errors (fail-closed).
  - The timeout defaults to `upstream.timeout_ms` (15000) and `opts.timeout_ms` overrides it per call. `opts.timeout_ms` is an upper bound: the effective upstream timeout is capped by the remaining script budget, with a small reply margin reserved so a timeout can surface as an upstream failure.
  - Redirects are followed manually, at most 3 hops; protocol and host allowlist are re-validated before every hop.
  - `status` and `headers` are snapshots; header names are lowercased and a repeated name keeps the last value. `text()` decodes UTF-8 lossily; `bytes()` returns a `Uint8Array`. Response bodies are capped at 8 MiB per call; larger or binary payloads belong to `ctx.http.pipe`.
  - Upstream requests are direct. Environment proxy variables (`HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, and lowercase variants) are not used.
  - Upstream 4xx/5xx responses are data and are never thrown. DNS, connection, TLS, and timeout failures throw a catchable error whose `error.code` is `"upstream_unreachable"`; if uncaught, the request returns 502 `upstream_unreachable` with a `request_id`.
- `ctx.http.pipe(url, opts)` streams one allowlisted upstream GET body straight to the client response; the bytes never enter the script heap.
  - `url` follows the same absolute-URL and allowlist rules as `ctx.http.get`. Redirects are followed manually, at most 3 hops, with protocol and host re-validation on every hop.
  - `opts` is optional and accepts only `{status, headers}`. `status` must be an integer in `[100, 599]`; `headers` accepts the same object or `[name, value]` pair shapes as `ctx.respond`. Unknown keys, non-plain objects, and malformed values are script errors (fail-closed).
  - `status` defaults to the upstream 2xx status, so a plain download answers `200` and a Range request answered with `206` keeps its partial-response status. Script-supplied headers are sent as given; upstream `Content-Range` and `Content-Length` are preserved unless the script sets a header with the same name.
  - The client's `Range` request header is forwarded to the upstream call.
  - Upstream responses in `[200, 299]` start the stream. A final non-2xx response raises a catchable error whose `error.code` is `"upstream_http_error"`; a URL that does not parse or whose scheme is not `http`/`https` raises `"upstream_url_invalid"`; a redirect chain the host cannot follow (more than 3 hops or an unusable `Location`) raises `"upstream_redirect_error"`; DNS, connection, TLS, and timeout failures raise `"upstream_unreachable"`; an allowlist rejection raises `"script_error"`.
  - The first `ctx.respond` or `ctx.http.pipe` call wins; a later call is ignored and produces a server-side warning. An uncaught `upstream_http_error` is an ordinary script error (500 `script_error`), never `502 upstream_unreachable`.
  - The upstream body read stays bounded by the effective upstream timeout, and a body already being streamed cannot be transformed or converted into a buffered response; scripts that need the bytes use `ctx.http.get`. Once the stream starts, a mid-body upstream failure can only truncate the client body, because the status and headers are already on the wire.
- `ctx.env` is the process environment snapshot. No `.env` file is loaded.
- `ctx.log.info` / `warn` / `error` write to server logs only and never to the client response.
- A script that throws, exceeds `sandbox.script_timeout_ms` (default 10000), or fails to load returns 500 `script_error`; a script that finishes without calling `ctx.respond` returns 500 `script_no_response`; an uncaught upstream transport failure returns 502 `upstream_unreachable`. All three carry a `request_id`.

## Security boundary

Scripts receive no raw `fetch`, `fs`, `os`, `subprocess`, or `socket`. All external capabilities come from host functions and are subject to:

- script timeout
- memory limit
- network allowlist
- static file root confinement
- upload size limit
- stack traces never returned to clients

## Type definitions

The product publishes `.d.ts` files for each supported `apiVersion`. The type file is part of the public contract.

The `apiVersion` 1 source definition for the currently implemented subset lives at [`types/ctx-api-v1.d.ts`](../../types/ctx-api-v1.d.ts). T8 (#11) publishes it with release artifacts and extends it as later capability groups land.

## Deprecation

- Warn at least one minor version before removal.
- A removed function requires a new `apiVersion`.
- Migration notes go into `CHANGELOG.md` and release notes.

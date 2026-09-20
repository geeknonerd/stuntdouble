# `ctx` host API contract

- Status: draft; slices T2–T4 implement the `apiVersion`, `request`, `http.get`, `respond`, `log`, and `env` subset
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
| Binary passthrough | `ctx.http.pipe` | not implemented |
| Files | `ctx.file.readText` / `ctx.file.readBytes` / `ctx.file.stream` | not implemented |
| Response | `ctx.respond` | implemented (T2) |
| Request-local state | `ctx.local` | not implemented |
| Logging | `ctx.log.info` / `warn` / `error` | implemented (T2) |
| Environment | `ctx.env` | implemented (T2) |
| Timers | `setTimeout` / `setInterval` | not implemented |

## Implemented subset (slices T2–T4)

- `ctx.apiVersion` is `"1"`.
- `ctx.request` is a read-only snapshot with `method`, `path`, `params`, `query`, `headers`, and `bodyText`. Header names are lowercased; `bodyText` is `null` when the request body is not valid UTF-8.
- `ctx.respond(status, headers, body)` accepts a status in `[100, 599]`, headers as an object or `[name, value]` pairs, and a body that is a string, byte array, or `Uint8Array`. The first call wins; later calls are ignored and produce a server-side warning.
- `ctx.http.get(url, opts)` performs an allowlisted upstream GET and returns `{status, headers, text(), bytes()}`.
  - `url` must be an absolute `http` or `https` URL whose host matches `upstream.allow_hosts` case-insensitively; the port is not part of the match, and IP literals and `localhost` require an explicit entry.
  - `opts` must be a plain object and accepts only `{ timeout_ms }`. Unknown string or symbol keys, inherited keys, non-object values, and explicit `null`, `NaN`, `Infinity`, non-integer, or non-positive `timeout_ms` values are script errors (fail-closed).
  - The timeout defaults to `upstream.timeout_ms` (15000) and `opts.timeout_ms` overrides it per call. `opts.timeout_ms` is an upper bound: the effective upstream timeout is capped by the remaining script budget, with a small reply margin reserved so a timeout can surface as an upstream failure.
  - Redirects are followed manually, at most 3 hops; protocol and host allowlist are re-validated before every hop.
  - `status` and `headers` are snapshots; header names are lowercased and a repeated name keeps the last value. `text()` decodes UTF-8 lossily; `bytes()` returns a `Uint8Array`. Response bodies are capped at 8 MiB per call; larger or binary payloads belong to `ctx.http.pipe` (T6).
  - Upstream requests are direct. Environment proxy variables (`HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, and lowercase variants) are not used.
  - Upstream 4xx/5xx responses are data and are never thrown. DNS, connection, TLS, and timeout failures throw a catchable error whose `error.code` is `"upstream_unreachable"`; if uncaught, the request returns 502 `upstream_unreachable` with a `request_id`.
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

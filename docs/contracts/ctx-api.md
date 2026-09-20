# `ctx` host API contract

- Status: draft; slice T2 implements the `apiVersion`, `request`, `respond`, `log`, and `env` subset
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
| Upstream HTTP | `ctx.http.get` / `ctx.http.request` | not implemented |
| Binary passthrough | `ctx.http.pipe` | not implemented |
| Files | `ctx.file.readText` / `ctx.file.readBytes` / `ctx.file.stream` | not implemented |
| Response | `ctx.respond` | implemented (T2) |
| Request-local state | `ctx.local` | not implemented |
| Logging | `ctx.log.info` / `warn` / `error` | implemented (T2) |
| Environment | `ctx.env` | implemented (T2) |
| Timers | `setTimeout` / `setInterval` | not implemented |

## Implemented subset (slice T2)

- `ctx.apiVersion` is `"1"`.
- `ctx.request` is a read-only snapshot with `method`, `path`, `params`, `query`, `headers`, and `bodyText`. Header names are lowercased; `bodyText` is `null` when the request body is not valid UTF-8.
- `ctx.respond(status, headers, body)` accepts a status in `[100, 599]`, headers as an object or `[name, value]` pairs, and a body that is a string, byte array, or `Uint8Array`. The first call wins; later calls are ignored and produce a server-side warning.
- `ctx.env` is the process environment snapshot. No `.env` file is loaded.
- `ctx.log.info` / `warn` / `error` write to server logs only and never to the client response.
- A script that throws, exceeds `sandbox.script_timeout_ms` (default 10000), or fails to load returns 500 `script_error`; a script that finishes without calling `ctx.respond` returns 500 `script_no_response`. Both carry a `request_id`.

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

## Deprecation

- Warn at least one minor version before removal.
- A removed function requires a new `apiVersion`.
- Migration notes go into `CHANGELOG.md` and release notes.

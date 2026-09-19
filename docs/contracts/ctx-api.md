# `ctx` host API contract

- Status: draft
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
| Request | `ctx.request` | draft |
| Upstream HTTP | `ctx.http.get` / `ctx.http.request` | draft |
| Binary passthrough | `ctx.http.pipe` | draft |
| Files | `ctx.file.readText` / `ctx.file.readBytes` / `ctx.file.stream` | draft |
| Response | `ctx.respond` | draft |
| Request-local state | `ctx.local` | draft |
| Logging | `ctx.log.info` / `warn` / `error` | draft |
| Environment | `ctx.env` | draft |
| Timers | `setTimeout` / `setInterval` | draft |

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

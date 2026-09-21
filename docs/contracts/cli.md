# CLI contract

**English** \| [中文](./cli.zh-CN.md)

- **Status**: stable for v0.x slice T7
- **Applies to**: v0.1.0-alpha.1 and later within the same configuration family
- **Stability**: breaking changes allowed before 1.0 with a deprecation window

## Commands

| Command | Purpose | Status |
| --- | --- | --- |
| `stuntdouble serve` | Start the mock server listening for HTTP requests | implemented |
| `stuntdouble validate` | Validate the configuration file without starting the server | implemented |

### serve

Usage:

```bash
stuntdouble serve --config <path> [--verbose]
```

Starts the mock server binding to the configured address. Matched routes execute their JavaScript in the embedded Boa runtime with a host-injected `ctx`, including allowlisted `ctx.http.get` calls and streaming `ctx.http.pipe` calls; unmatched routes answer 404 `not_found`. Script failures answer 500 with `script_error` or `script_no_response`; an uncaught upstream transport failure answers 502 `upstream_unreachable`.

`--verbose` attaches a `detail` field to engine-generated 500/502 JSON error bodies. The value is a stable failure class, for example `script execution failed`, `script exceeded the configured timeout`, `upstream transport failure: timeout`, `upstream transport failure: dns`, or `upstream transport failure: transport`. It never contains stack traces, script messages, upstream bodies, hostnames, IP addresses, or URLs. Without the flag, error bodies contain only `error` and `request_id`. Use it for local diagnosis only; do not enable it in shared environments.

Default config path is `stuntdouble.toml`. Configuration must include `[[routes]]`; each route specifies method, path, script location, optional name.

#### Exit codes

- `0`: successfully started (server runs until shutdown signal)
- `2`: configuration error (bad TOML or schema violation) — messages printed to stderr
- `3`: internal/server startup failure

#### Binding semantics

The `server.bind` field must be an IP address literal (`127.0.0.1` etc.). Hostnames are resolved by OS but not supported directly in this slice. `server.port` defaults to 3000 when omitted.

#### Request logging

`serve` writes one JSON log line per request to stderr. The line contains `request_id`, matched `route`, `method`, `path`, `params`, `status`, `error`, `elapsed_ms`, `script_duration_ms`, `upstream_calls`, `request_body_bytes`, `response_body_bytes`, `request_headers`, `response_headers`, `client_request_id`, `host`, and `script_logs` when the script logged anything.

- `upstream_calls` is an ordered list of the calls the script made. Each entry records `api` (`http.get` or `http.pipe`), `host`, `path`, `status`, `duration_ms`, `redirects`, and the stable `error`/`kind` when the call failed. Query strings are not logged.
- Request and response bodies are never logged. Only sizes and allowlisted headers are recorded: request headers `accept`, `content-type`, `content-length`, `range`, `user-agent`; response headers `content-type`, `content-length`, `content-range`. Authorization, cookie, and other headers are never logged.
- `response_body_bytes` is `null` for a streamed response without a known `Content-Length`.
- A call still in flight when the script deadline hits keeps `null` for `status` and `duration_ms`; calls that finished before the deadline keep their recorded values.
- `client_request_id` records a client-supplied `X-Request-ID`. It is never adopted as `request_id` and is never forwarded to an upstream call.

### validate

Usage:

```bash
stuntdouble validate --config <path>
```

Loads the configuration file, validates required fields and types, prints diagnostic details to stderr and "OK" to stdout. Always exits before opening sockets. If the configuration is invalid, the command prints violations with dotted field names, expected shapes, and actual values.

Exit code `2` when the configuration fails. `0` otherwise.

## Global flags

The `-c, --config` flag is global and applies to all commands:

- `stuntdouble --config x serve` — equivalent to `stuntdouble serve --config x`
- `stuntdouble --config x validate` — equivalent to `stuntdouble validate --config x`

| Flag | Purpose | Status |
| --- | --- | --- |
| `-c, --config <path>` | Select a configuration file (default `stuntdouble.toml`) | implemented |
| `-h, --help` | Print help | implemented |
| `-V, --version` | Print version metadata (`stuntdouble <version>`) | implemented |

## Exit codes

Used by every command except `serve` runtime exit.

| Code | Meaning |
| ---: | --- |
| `0` | Success |
| `1` | Runtime error (only reported by serve while running) |
| `2` | Configuration error |
| `3` | Internal error |

## Deprecation

- Warn at least one minor version before removing or renaming a flag or command.
- Warnings go to stderr and `CHANGELOG.md`.
- `1.0` and later follow SemVer for CLI compatibility.

# CLI contract

**English** \| [中文](./cli.zh-CN.md)

- **Status**: frozen for the v1 slice (T1–T8)
- **Applies to**: the `stuntdouble` binary from v0.1.0-alpha.1 onward within the same CLI family
- **Stability**: breaking changes before 1.0 need a deprecation window

## Commands

| Command | Purpose | Status |
| --- | --- | --- |
| `stuntdouble serve` | Start the mock server listening for HTTP requests | implemented |
| `stuntdouble validate` | Validate the configuration file without starting the server | implemented |

### serve

Usage:

```bash
stuntdouble serve [--config <path>] [--verbose]
```

Starts the mock server binding to the configured address. Matched Routes execute their JavaScript in the embedded Boa runtime with a host-injected `ctx`, including allowlisted `ctx.http.get` calls and streaming `ctx.http.pipe` calls; unmatched Routes answer 404 `not_found`. Script failures answer 500 `script_error` or `script_no_response`; an uncaught upstream transport failure answers 502 `upstream_unreachable`.

`--verbose` attaches a `detail` field to engine-generated 500/502 JSON error bodies. The value is a stable failure class, for example `script execution failed`, `script exceeded the configured timeout`, `upstream transport failure: timeout`, `upstream transport failure: dns`, or `upstream transport failure: transport`. It never contains stack traces, script messages, upstream bodies, hostnames, IP addresses, or URLs. Without the flag, error bodies contain only `error` and `request_id`. Use it for local diagnosis only; do not enable it in shared environments. A 404 `not_found` response never carries `detail`.

Default config path is `stuntdouble.toml`. The configuration must contain a non-empty `routes` array; each Route specifies method, path, script location, and an optional name. Full schema rules live in the [configuration contract](config.md).

#### serve exit codes

| Code | Meaning |
| ---: | --- |
| `0` | the server returned without an error; `validate`, `--help`, and `--version` also use `0` for success |
| `1` | runtime/server error, including a socket bind or listen failure |
| `2` | configuration error (bad TOML, schema violation, unknown `config_version`, or non-IP `server.bind`) or CLI usage error (unknown flag or missing subcommand) — messages print to stderr |
| `3` | internal error while constructing the async runtime |

#### Shutdown signals

The v1 slice does not install a graceful-shutdown signal handler. SIGINT or SIGTERM terminates the process by signal, so a shell commonly reports 130 or 143 rather than `0`. `0` is reserved for a normal server return. Graceful shutdown is tracked in [#33](https://github.com/geeknonerd/stuntdouble/issues/33).

#### Binding semantics

The `server` table is required. Its `bind` field is optional and defaults to `127.0.0.1`; `port` is optional and defaults to `3000`.

`server.bind` must be an IP address literal (`127.0.0.1`, `::1`, and so on). Hostnames are rejected during configuration validation with exit code `2`; the CLI does not resolve hostnames in this slice.

#### Request logging

`serve` writes one JSON log line per request to stderr. The line contains `request_id`, matched `route`, `method`, `path`, `params`, `status`, `error`, `elapsed_ms`, `script_duration_ms`, `upstream_calls`, `request_body_bytes`, `response_body_bytes`, `request_headers`, `response_headers`, `client_request_id`, `host`, and `script_logs` when the script logged anything.

- `upstream_calls` is an ordered list of the calls the script made. Each entry records `api` (`http.get` or `http.pipe`), `host`, `path`, `status`, `response_bytes`, `duration_ms`, `redirects`, and the stable `error`/`kind` when the call failed. Query strings are not logged. A `http.pipe` entry is finalized when its body stream ends.
- The host never logs request or response bodies automatically. Only sizes and allowlisted headers are recorded: request headers `accept`, `content-type`, `content-length`, `range`, `user-agent`; response headers `content-type`, `content-length`, `content-range`. Authorization, cookie, and other headers are never logged. `script_logs[].message` is script-authored and not redacted: `ctx.log.*` must not carry bodies, tokens, cookies, or other secrets.
- Buffered Responses log before the Response is written. A streamed Response (`ctx.http.pipe`) logs one completion line after the body ends or the client disconnects: `response_body_bytes` counts the bytes relayed into the Response body, and `error` can be `upstream_stream_error` (the upstream read failed after a 2xx status was already sent, so the client status stays 2xx) or `client_disconnected`. `elapsed_ms` covers the whole stream in that line.
- A `http.get` call still in flight when the script deadline hits keeps `null` for `status`, `response_bytes`, and `duration_ms`; calls that finished before the deadline keep their recorded values.
- These lines are operator-facing diagnostics: they can contain allowlisted upstream hosts and paths. Redact them before publishing them in issues, pull requests, or other public artifacts.
- `client_request_id` records a client-supplied `X-Request-ID`. It is never adopted as `request_id` and is never forwarded to an upstream call.

### validate

Usage:

```bash
stuntdouble validate [--config <path>]
```

Loads the configuration file and validates required fields, types, defaults, and the filesystem checks in the [configuration contract](config.md). It always exits before opening sockets and never reads Route scripts.

On success, the command prints `<path>: valid configuration` to stdout and a short summary (`config_version`, server address, `files.root`, and Route count) to stderr. On failure, it prints the file path and every violation to stderr, including dotted field names, expected shapes, and actual values.

Exit code `2` when the configuration is invalid or cannot be read, or when CLI arguments are invalid; `0` otherwise.

## Global flags

The `-c, --config` flag is global and applies to all commands:

- `stuntdouble --config x serve` is equivalent to `stuntdouble serve --config x`
- `stuntdouble --config x validate` is equivalent to `stuntdouble validate --config x`

| Flag | Purpose | Status |
| --- | --- | --- |
| `-c, --config <path>` | Select a configuration file (default `stuntdouble.toml`) | implemented |
| `-h, --help` | Print help | implemented |
| `-V, --version` | Print version metadata (`stuntdouble <version>`) | implemented |

## Exit codes

Used by every command:

| Code | Meaning |
| ---: | --- |
| `0` | success; `serve` uses it only for a normal return, not for signal termination |
| `1` | runtime/server error (reported by `serve`) |
| `2` | configuration error or CLI usage error |
| `3` | internal error |

## Deprecation

- Warn for at least one minor version before removing or renaming a flag or command.
- Warnings go to stderr and `CHANGELOG.md`.
- `1.0` and later follow SemVer for CLI compatibility.

# CLI contract

- **Status**: stable for v0.x slice T6
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

Starts the mock server binding to the configured address. Matched routes execute their JavaScript in the embedded Boa runtime with a host-injected `ctx`, including allowlisted `ctx.http.get` calls and streaming `ctx.http.pipe` calls; unmatched routes answer 404 `not_found`. Script failures answer 500 with `script_error` or `script_no_response`; an uncaught upstream transport failure answers 502 `upstream_unreachable`. The `--verbose` flag is currently accepted but does not emit extra diagnostics; full detail payloads land in a later slice.

Default config path is `stuntdouble.toml`. Configuration must include `[[routes]]`; each route specifies method, path, script location, optional name.

#### Exit codes

- `0`: successfully started (server runs until shutdown signal)
- `2`: configuration error (bad TOML or schema violation) — messages printed to stderr
- `3`: internal/server startup failure

#### Binding semantics

The `server.bind` field must be an IP address literal (`127.0.0.1` etc.). Hostnames are resolved by OS but not supported directly in this slice. `server.port` defaults to 3000 when omitted.

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

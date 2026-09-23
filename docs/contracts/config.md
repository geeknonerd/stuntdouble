# Configuration contract

**English** \| [中文](./config.zh-CN.md)

- **Status**: frozen for the v1 slice (T1–T8)
- **Applies to**: configurations that declare `config_version = "1"`
- **Stability**: within version `"1"`, fields may be added; breaking changes before 1.0 need a deprecation window

## Format

The primary configuration format is TOML. The default file is `stuntdouble.toml` in the current working directory. Use `--config <path>` to select another file.

YAML and JSON are not accepted in v1.

## Schema

Every valid configuration is a TOML table. Unknown keys, unknown `config_version` values, wrong value shapes, and missing required fields are fail-closed configuration errors.

### Top-level keys

| Key | Required | Type | Default | Validation |
| --- | --- | --- | --- | --- |
| `config_version` | yes | string | — | exactly `"1"`; any other value exits with code `2` |
| `server` | yes | table | — | only `{bind, port}` is accepted |
| `server.bind` | no | string | `"127.0.0.1"` | IP address literal; hostnames are rejected during validation |
| `server.port` | no | integer | `3000` | integer in `[1, 65535]` |
| `files` | yes | table | — | only `{root}` is accepted |
| `files.root` | yes | string | — | existing directory, resolved relative to the configuration file |
| `sandbox` | no | table | — | only `{script_timeout_ms}` is accepted |
| `sandbox.script_timeout_ms` | no | integer | `10000` | positive integer (0 is rejected) |
| `upstream` | no | table | — | only `{allow_hosts, timeout_ms}` is accepted |
| `upstream.allow_hosts` | no | array of strings | `[]` | URL host syntax; entries carry no scheme or port, and the default denies every host |
| `upstream.timeout_ms` | no | integer | `15000` | positive integer (0 is rejected) |
| `routes` | yes | array of tables | — | must contain at least one Route table; `routes = []` is invalid |

Domains in `upstream.allow_hosts` are lowercased/punycoded. IPv6 literals are written in brackets (`"[::1]"`) and normalized to their unbracketed form at load time. Hosts are matched case-insensitively without the port, and IP literals and `localhost` must be listed explicitly.

### Route schema

| Field | Required | Type | Validation |
| --- | --- | --- | --- |
| `name` | no | string | non-empty; used in logs and diagnostics, otherwise the `path` is used |
| `method` | yes | string | one of `GET`, `POST`, `PUT`, `DELETE`, `PATCH`, `HEAD`, `OPTIONS` (case-insensitive, normalized to uppercase) |
| `path` | yes | string | absolute path starting with `/`; `:param` segments are captured by name |
| `script` | yes | string | file name ending in `.js`, `.mjs`, or `.cjs`; relative paths resolve against the configuration file's parent, absolute paths are accepted |

`validate` checks the `script` extension but does not read the script file. The file is loaded per request; if it is missing or cannot be read when a Route is matched, the request returns 500 `script_error`.

## Example

```toml
config_version = "1"

[server]
bind = "127.0.0.1"        # IP literal
port = 3000               # integer in [1, 65535]

[files]
root = "./files"          # existing directory

[sandbox]                 # optional table
script_timeout_ms = 10000 # positive integer; default 10000

[upstream]                # optional table
allow_hosts = ["metadata.example.com"] # exact host allowlist; default []
timeout_ms = 15000        # positive integer; default 15000

[[routes]]
name = "manifest"         # optional
method = "GET"            # one of GET, POST, PUT, DELETE, PATCH, HEAD, OPTIONS
path = "/demo/..."        # path starting with '/', :param segments supported
script = "scripts/x.js"   # .js/.mjs/.cjs; relative or absolute
```

## Route execution model

Routes follow the single pipeline: `match → source → transform → response`. This slice implements Match plus the script Transform: a matched Route runs its JavaScript, and the script produces the Response through `ctx.respond` or `ctx.http.pipe`. Unmatched requests return 404 `not_found`; a script that throws, times out, or fails to load returns 500 `script_error`; a script that finishes without producing a Response returns 500 `script_no_response`. `ctx.http.get` treats upstream responses (including 4xx/5xx) as data; `ctx.http.pipe` streams a 2xx upstream Response to the client and raises a catchable `upstream_http_error` for a final non-2xx answer; an uncaught upstream transport failure returns 502 `upstream_unreachable`. Local static-file reads are implemented; uploads arrive in a later slice.

### Matching semantics

- Method matching is case-insensitive and normalized to uppercase.
- Paths are split by `/` into segments; `:name` captures one segment as `{name: value}`.
- The query string does not participate in matching.
- First declaration wins; wildcards and regex are not supported in this slice.

## Validation

`stuntdouble validate` reports:

- the configuration file path
- dotted field paths (for example, `routes[0].method`)
- the expected shape and actual value
- line and column when the TOML parser provides them

Validation fails closed on unknown keys at every table level. It also rejects an unknown `config_version`, a non-IP `server.bind`, an empty `routes` array, non-positive timeout values, and a missing or non-directory `files.root`. `validate` never opens sockets and never reads Route scripts.

Configuration errors exit with code `2`.

## Deprecation

- Warn for at least one minor version before removing or renaming a field.
- Warnings go to server logs and `CHANGELOG.md`.
- Breaking changes include a migration example in release notes.
- `1.0` and later follow SemVer for configuration compatibility.

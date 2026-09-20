# Configuration contract

- **Status**: stable for v0.x slice T4; `config_version` enables future migrations
- **Applies to**: v0.1.0-alpha.1 and later within the same configuration family
- **Stability**: breaking changes allowed before 1.0 with a deprecation window

## Format

The primary configuration format is TOML. The default file is `stuntdouble.toml` in the current working directory. Use `--config <path>` to select another file.

YAML and JSON are not accepted in v1.

## Required fields

Every valid configuration must declare:

```toml
config_version = "1"

[server]
bind = "127.0.0.1"        # numeric IP address only in this slice
port = 3000              # integer in [1, 65535]

[files]
root = "./files"         # existing directory (relative to config parent resolved)

[sandbox]                # optional; the whole table is optional
script_timeout_ms = 10000 # positive integer; default 10000

[upstream]               # optional; the whole table is optional
allow_hosts = ["metadata.example.com"] # exact host allowlist; default []
timeout_ms = 15000       # positive integer; default 15000

[[routes]]
name = "manifest"       # optional name for logging and diagnostics
method = "GET"          # one of GET, POST, PUT, DELETE, PATCH, HEAD, OPTIONS
path = "/demo/..."      # path starting with '/', :param segments supported
script = "scripts/x.js" # relative or absolute path; .js/.mjs/.cjs accepted
```

All known keys must belong to `{config_version, server, files, sandbox, upstream, routes}`. Unknown top-level keys cause validation error (fail-closed). Fields inside `routes[]` tables must be subset of `{name, method, path, script}`; `[sandbox]` accepts only `{script_timeout_ms}`, a positive integer (0 is rejected). `[upstream]` accepts only `{allow_hosts, timeout_ms}`: `allow_hosts` is an array of host names or IP literals using URL host syntax (default `[]`, which denies every host). Entries carry no scheme or port; domains are lowercased/punycoded and IPv6 literals are written in brackets (`"[::1]"`) but normalized to their unbracketed form at load time. Hosts are matched case-insensitively without the port, and IP literals and `localhost` must be listed explicitly. `timeout_ms` is a positive integer (default 15000). `script` must end in `.js`, `.mjs`, or `.cjs`; `.py` is rejected with an explicit "not supported in this slice" message.

## Route execution model

Routes follow the single pipeline: `match → source → transform → response`. This slice implements match plus the script transform: a matched route runs its JavaScript, and the script produces the response through `ctx.respond`. Unmatched requests return 404 `not_found`; a script that throws, times out, or fails to load returns 500 `script_error`; a script that finishes without `ctx.respond` returns 500 `script_no_response`. This slice adds `ctx.http.get`: upstream responses (including 4xx/5xx) are data, and an uncaught transport failure returns 502 `upstream_unreachable`. File/binary responses arrive in later slices.

### Matching semantics

- Method case-insensitive, normalized to uppercase.
- Path split by '/' into segments; `:name` captures a segment as `{name: value}`.
- Query string not part of matching.
- First declaration wins; no wildcards or regex in this slice.

## Validation

`stuntdouble validate` reports:

- file path where errors occurred
- dotted field paths (e.g., `routes[0].method`)
- expected shape and actual value
- line/column when available from TOML parser

Configuration errors exit with code `2`.

## Deprecation

- Warn at least one minor version before removing or renaming a field.
- Warnings go to server logs and `CHANGELOG.md`.
- Breaking changes include a migration example in release notes.
- `1.0` and later follow SemVer for configuration compatibility.

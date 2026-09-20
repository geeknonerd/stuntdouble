# Configuration contract

- **Status**: stable for v0.x slice T1; `config_version` enables future migrations
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

[[routes]]
name = "manifest"       # optional name for logging and diagnostics
method = "GET"          # one of GET, POST, PUT, DELETE, PATCH, HEAD, OPTIONS
path = "/demo/..."      # path starting with '/', :param segments supported
script = "scripts/x.js" # relative or absolute path; .js extension required
```

All known keys must belong to `{config_version, server, files, routes}`. Unknown top-level keys cause validation error (fail-closed). Fields inside `routes[]` tables must be subset of `{name, method, path, script}`.

## Route execution model

Routes follow the single pipeline: `match → source → transform → response`. This slice implements only match; unmatched requests return 404, matched routes return 501 until transform logic is added in subsequent slices.

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

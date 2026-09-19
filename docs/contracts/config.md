# Configuration contract

- Status: draft
- Applies to: v0.x
- Stability: breaking changes allowed before 1.0 with a deprecation window

## Format

The primary configuration format is TOML. The default file is `stuntdouble.toml` in the current working directory. Use `--config <path>` to select another file.

YAML and JSON are not accepted in v1.

## Required fields

TBD. The first implementation slice must define:

- `config_version`
- server bind address and port
- static file root
- route list

## Routes

Each route follows the single execution model:

```text
match → source → transform → response
```

Route schema is TBD. Do not treat an unpublished schema as stable.

## Deprecation

- Warn at least one minor version before removing or renaming a field.
- Warnings go to server logs and `CHANGELOG.md`.
- Breaking changes include a migration example in release notes.
- `1.0` and later follow SemVer for configuration compatibility.

## Validation

`stuntdouble validate` must report:

- file path
- line and column where available
- expected shape
- actual value

Configuration errors exit with code `2`.

# CLI contract

- Status: draft
- Applies to: v0.x
- Stability: breaking changes allowed before 1.0 with a deprecation window

## Commands

| Command | Purpose | Status |
| --- | --- | --- |
| `stuntdouble serve` | Start the mock server | draft |
| `stuntdouble validate` | Validate the configuration file | draft |
| `stuntdouble version` | Print version, build information, and license | draft |
| `stuntdouble init` | Generate a starter `stuntdouble.toml` | draft |

## Global flags

| Flag | Purpose | Status |
| --- | --- | --- |
| `-c, --config <path>` | Select a configuration file | draft |
| `-h, --help` | Print help | draft |
| `-V, --version` | Print version | draft |

## Exit codes

| Code | Meaning |
| ---: | --- |
| `0` | Success |
| `1` | Runtime error |
| `2` | Configuration error |
| `3` | Internal error |

## Deprecation

- Warn at least one minor version before removing or renaming a flag or command.
- Warnings go to stderr and `CHANGELOG.md`.
- `1.0` and later follow SemVer for CLI compatibility.

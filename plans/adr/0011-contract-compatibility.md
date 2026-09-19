# ADR 0011: Separate compatibility contracts for config, ctx API, and CLI

- Status: Accepted
- Date: 2026-09-19
- Related: [ADR 0003](0003-script-first-multi-runtime.md), [ADR 0004](0004-host-functions-only-sandbox.md), [ADR 0010](0010-git-and-release-workflow.md)

## Context

Stunt Double exposes three public surfaces: the configuration file, the script host API (`ctx`), and the CLI. They evolve at different speeds. A single product version cannot describe all three safely, and removing host functions would break user scripts even when the server itself is compatible.

## Decision

Treat the three surfaces as separate contracts.

### Configuration

- Primary format is TOML.
- Default file is `stuntdouble.toml`; `--config <path>` overrides it.
- `0.x` releases may change the schema with a deprecation window.
- `1.0` and later follow SemVer for backward compatibility.
- Schema details live in `docs/contracts/config.md`.

### `ctx` host API

- Every script sees `ctx.apiVersion`.
- Version 1 is the first contract.
- Within an `apiVersion`, host functions may be added but not removed or renamed.
- Removing or renaming a function requires a new `apiVersion`.
- The product may support multiple `apiVersion` values.
- The matching `.d.ts` file is part of the contract.
- Details live in `docs/contracts/ctx-api.md`.

### CLI

- Commands, flags, and exit codes are public contracts.
- Exit codes: `0` success, `1` runtime error, `2` configuration error, `3` internal error.
- `0.x` releases may change the CLI with a deprecation window.
- `1.0` and later follow SemVer.
- Details live in `docs/contracts/cli.md`.

## Deprecation rule

Warn at least one minor version before removing or renaming a public surface. Warnings go to logs and `CHANGELOG.md`. Breaking changes include migration examples in release notes.

## Consequences

- Contract documents must be updated with the implementation, not after it.
- The `.d.ts` publishing step is part of the release checklist.
- `1.0.0` requires stable config, CLI, and `ctx` API version 1.

# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- First implementation slice (T1): Cargo project with clap CLI (`serve`/`validate`), TOML config loader with fail-closed schema, route matching (method + path/:param), HTTP 404/501 responses with request_id correlation, and per-request structured logging.
- `stuntdouble validate` command checks configuration syntax and semantics without binding sockets; reports violations with dotted field paths, expected shapes, and actual values.
- Global `-c/--config` flag usable before or after the subcommand (`stuntdouble --config x serve` and `stuntdouble serve --config x`); defaults to `stuntdouble.toml`.
- `stuntdouble serve` starts the mock server, accepts numeric bind addresses (127.0.0.1 default) and port (3000 default). Unmatched routes return 404; matched routes return 501 `script_unimplemented` pending transform slices.
- Each request generates unique request_id (non-crypto), logged per-request and included in error responses (404/501) via body and `X-Request-ID` header.
- CI gates activated for fmt, clippy (pedantic), tests, docs, deny, audit, MSRV (1.82 candidate).
- Bin+lib crate structure enables `cargo test --doc`: src/lib.rs exports config/matcher/server modules; src/main.rs imports from stuntdouble::{config,server}. See [docs/solutions/ci/doctest-lib-target-required.md](docs/solutions/ci/doctest-lib-target-required.md).

### Changed
- Contract documents marked stable for v0.x slice T1: `docs/contracts/config.md` now includes complete route schema and validation rules; `docs/contracts/cli.md` documents implemented commands and exit codes.


- Open-source repository baseline: README, contribution guide, security policy, code of conduct, issue templates, and pull request template.
- Dual license: MIT OR Apache-2.0.
- GitHub Pages landing page.
- Git workflow, versioning, release, compatibility, and supply-chain decision records.
- Draft contracts for configuration, `ctx` API, and CLI.
- Governance and development guides.
- CI, PR title, and DCO workflows.
- Dependabot configuration for Cargo and GitHub Actions.
- Repository labels for triage, area, status, and security.

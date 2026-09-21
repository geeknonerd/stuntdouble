# Stunt Double

**English** \| [中文](README.zh-CN.md)

> **A test double that plays the whole show.**

Stunt Double is a Rust-based mock server for integration testing against real external dependencies. It reads upstream APIs, transforms data with built-in JavaScript or Python, returns files and binary responses, and runs with zero host runtime dependencies.

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Status: upstream HTTP](https://img.shields.io/badge/status-upstream%20http-orange.svg)](#status)

## Why Stunt Double

Most mock tools are optimized for static stubs. Stunt Double targets the part of integration work that static stubs usually cannot cover:

- Upstream APIs that return the data your client actually needs.
- CSV, JSON, text, and binary transformations.
- PDF, file, and byte-stream responses with Range support.
- CI environments where installing Node.js, Python, or a JVM is not acceptable.
- Scripted behavior with explicit host limits instead of an unrestricted runtime.

The design goal is a single binary that behaves like the real dependency closely enough to test the client, not just to return a canned response.

## Status

**Script execution and upstream HTTP (T6) implemented.** `stuntdouble serve` and `stuntdouble validate` run from a TOML configuration, match routes by method and path (`:param` capture), and execute each matched route's JavaScript in the embedded Boa runtime with a host-injected `ctx` (`apiVersion`, `request`, `http.get`, `http.pipe`, `respond`, `log`, `env`). Both upstream calls reach only hosts listed in `[upstream] allow_hosts`: `ctx.http.get` treats upstream 4xx/5xx responses as data and maps uncaught transport failures to 502 `upstream_unreachable`, while `ctx.http.pipe` streams a 2xx body to the client around the script heap, forwards `Range`, and keeps 206 `Content-Range`. Unmatched routes answer 404 `not_found`; scripts that throw or time out answer 500 `script_error`, and scripts that never call `ctx.respond` answer 500 `script_no_response` — always with a `request_id`. The document manifest and PDF download scenarios from `plans/demo-document-catalog.md` run from the in-repo fixture in [`demo/`](demo/README.md). Local static files, uploads, and file responses are the next slices.

The execution model and security boundaries were fixed before implementation details. See [plans/adr/](plans/adr/) and [docs/solutions/](docs/solutions/) (both in Chinese).

```bash
cargo install --path .   # or: cargo run -- serve --config stuntdouble.toml
```

## Planned v1

- Route model with one pipeline: `match → source → transform → response`.
- Built-in JavaScript runtime: Boa.
- Built-in Python runtime: RustPython with a stdlib subset.
- Host-injected `ctx` API. No raw `fetch`, `fs`, `os`, `subprocess`, or `socket`.
- Upstream HTTP (`ctx.http.get` plus streaming `ctx.http.pipe` with Range passthrough; `ctx.http.request` pending), static file reads, file streaming, uploads, and local file Range responses.
- One static file root with path traversal protection.
- Static configuration with restart. Hot reload and Admin API are deferred.
- Linux x86_64, macOS arm64, and Windows x86_64 binaries plus a container image.
- Structured request logs with `request_id` and stable error classes.

## Non-goals for v1

- Request-to-request shared state.
- Response sequencing.
- Automatic resource CRUD.
- TypeScript transpilation.
- npm, pip, or third-party imports.
- Hot reload, Admin API, or GUI.
- Built-in TLS termination.
- WebSocket, GraphQL, or gRPC.

See [plans/product-definition.md](plans/product-definition.md) for the full scope.

## Documentation

- [Getting started](docs/guide/getting-started.md) — install, first route, and upstream calls.
- [Public contracts](docs/contracts/) — configuration, `ctx` API, and CLI.
- [Demo fixture](demo/README.md) — runnable configuration, script, and error mapping for the CSV manifest.
- [Documentation site](https://geeknonerd.github.io/stuntdouble/) — the same contracts and demo entry points, rendered.
- [Changelog](CHANGELOG.md) — release history.
- [Contributing](CONTRIBUTING.md) · [Governance](GOVERNANCE.md) · [Security](SECURITY.md) · [Code of Conduct](CODE_OF_CONDUCT.md)

Development documentation is written in Chinese; [ADR 0013](plans/adr/0013-documentation-language-and-bilingual-structure.md) records the language policy.

- [Product definition](plans/product-definition.md) (Chinese) — v1 scope, host API, non-goals.
- [Demo scenario](plans/demo-document-catalog.md) (Chinese) — CSV manifest and PDF download contract.
- [Development and release workflow](docs/development.md) (Chinese) — branch, commit, CI, versioning, and release rules.
- [Architecture decisions](plans/adr/) (Chinese) — ADR 0001–0013.
- [Domain glossary](CONTEXT.md) (Chinese) — project vocabulary.
- [Research](research/) (Chinese) — mock server landscape, runtimes, architecture, and open-source baseline.
- [Repository documentation index](docs/README.md) (Chinese) — entry point for maintainers.

## Contributing

Before proposing a feature, read the product definition and the ADRs. The project has a deliberately narrow v1 scope, and new capabilities must fit the single execution model.

- Follow the [Code of Conduct](CODE_OF_CONDUCT.md).
- Use the GitHub issue templates for bugs and feature requests.
- Keep customer data out of issues, logs, fixtures, and screenshots.
- Sign off commits with `git commit -s` (DCO).
- Keep commit messages in English.

See [CONTRIBUTING.md](CONTRIBUTING.md) and [docs/development.md](docs/development.md) for the full workflow.

## Security

Do not report vulnerabilities in a public issue. Use GitHub private vulnerability reporting for this repository. See [SECURITY.md](SECURITY.md).

Never paste customer hostnames, tokens, headers, production logs, request bodies, or response bodies into a public issue.

## License

Dual-licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

You may choose either license. This is the standard permissive licensing pattern for Rust projects and keeps the core usable in commercial and open-source environments.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this project is dual-licensed as above, without additional terms or conditions.

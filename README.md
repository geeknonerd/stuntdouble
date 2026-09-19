# Stunt Double

> **A test double that plays the whole show.**

Stunt Double is a planned Rust-based mock server for integration testing against real external dependencies. It is designed to read upstream APIs, transform data with built-in JavaScript or Python, return files and binary responses, and run with zero host runtime dependencies.

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Status: design phase](https://img.shields.io/badge/status-design%20phase-orange.svg)](#status)

## Why Stunt Double

Most mock tools are optimized for static stubs. Stunt Double targets the part of integration work that static stubs usually cannot cover:

- Upstream APIs that return the data your client actually needs.
- CSV, JSON, text, and binary transformations.
- PDF, file, and byte-stream responses with Range support.
- CI environments where installing Node.js, Python, or a JVM is not acceptable.
- Scripted behavior with explicit host limits instead of an unrestricted runtime.

The design goal is a single binary that behaves like the real dependency closely enough to test the client, not just to return a canned response.

## Status

**Design phase.** There is no runnable server yet. The repository currently contains product definition, architecture decisions, and a public demo scenario.

The project is intentionally starting with the execution model and security boundaries before choosing implementation details. See [plans/adr/](plans/adr/).

## Planned v1

- Route model with one pipeline: `match → source → transform → response`.
- Built-in JavaScript runtime: Boa.
- Built-in Python runtime: RustPython with a stdlib subset.
- Host-injected `ctx` API. No raw `fetch`, `fs`, `os`, `subprocess`, or `socket`.
- Upstream HTTP, static file reads, file streaming, uploads, and Range responses.
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

- [Product definition](plans/product-definition.md) — v1 scope, host API, non-goals.
- [Demo scenario](plans/demo-document-catalog.md) — CSV manifest and PDF download contract.
- [Architecture decisions](plans/adr/) — ADR 0001–0009.
- [Research](research/) — mock server landscape, runtimes, architecture, and open-source baseline.
- [Chinese README](README.zh-CN.md).

Most design documents are currently written in Chinese. English translations are welcome; see [CONTRIBUTING.md](CONTRIBUTING.md).

## Contributing

Before proposing a feature, read the product definition and the ADRs. The project has a deliberately narrow v1 scope, and new capabilities must fit the single execution model.

- Use the GitHub issue templates for bugs and feature requests.
- Keep customer data out of issues, logs, fixtures, and screenshots.
- Sign off commits with `git commit -s` (DCO).
- Keep commit messages in English.

See [CONTRIBUTING.md](CONTRIBUTING.md) for the full workflow.

## Security

Do not report vulnerabilities in a public issue. Use GitHub private vulnerability reporting for this repository. See [SECURITY.md](SECURITY.md).

Never paste customer hostnames, tokens, headers, production logs, request bodies, or response bodies into a public issue.

## License

Dual-licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

You may choose either license. This is the standard permissive licensing pattern for Rust projects and keeps the core usable in commercial and open-source environments.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this project is dual-licensed as above, without additional terms or conditions.

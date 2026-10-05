# Stunt Double

**English** | [中文](README.zh-CN.md)

> **A test double that plays the whole show.**

Stunt Double is a Rust-based mock server for integration testing against real external dependencies. It reads upstream APIs, transforms data with built-in JavaScript, returns files and binary responses, and runs with zero host runtime dependencies.

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Release](https://img.shields.io/github/v/release/geeknonerd/stuntdouble)](https://github.com/geeknonerd/stuntdouble/releases)
[![CI](https://github.com/geeknonerd/stuntdouble/actions/workflows/ci.yml/badge.svg)](https://github.com/geeknonerd/stuntdouble/actions/workflows/ci.yml)

v1.0.1 is released; the configuration, `ctx` API, and CLI contracts are frozen. The execution model and security boundaries live in [plans/adr/](plans/adr/); details live in [docs/](docs/README.md).

## Quickstart (60 seconds)

Run the offline demo from a checkout — no new files, no network:

```bash
cargo run -- serve --config demo/stuntdouble.toml
curl -i http://127.0.0.1:3000/demo/documents/local-manifest/group-a
```

Expect `200` with a CSV body:

```text
文件编码,文件标题,系统代码
DOC-0001,示例设备 A 安装手册,SYS-A
DOC-0002,示例设备 B 运行手册,SYS-B
```

Next: [Getting started](docs/guide/getting-started.md) builds your first route from scratch; [Mock recipes](docs/guide/mock-recipes.md) covers upstream, file, and upload flows.

## Why Stunt Double

Most mock tools are optimized for static stubs. Stunt Double targets the integration work static stubs cannot cover:

- Upstream APIs that return the data your client actually needs.
- CSV, JSON, text, and binary transformations via scripts.
- PDF, file, and byte-stream responses with Range support.
- CI environments where installing Node.js, Python, or a JVM is not acceptable.

| Tool | One-line position | Choose Stunt Double when you need |
|---|---|---|
| WireMock | Java-based general stubbing | Rust single binary plus upstream rewrite and binary streaming |
| Mockoon | Local static stubs and GUI | Scripted transforms plus Range/206 and zero-runtime CI |
| json-server | Resource-derived CRUD | Per-route declarations plus upstream and a file root |
| Prism | OpenAPI contract validation | A stand-in that reshapes real data before returning it |

## Install

Download the archive for your platform from the [releases page](https://github.com/geeknonerd/stuntdouble/releases), then verify its provenance:

```bash
gh attestation verify stuntdouble-x86_64-unknown-linux-gnu.tar.gz --repo geeknonerd/stuntdouble
```

Or run the container image (set `server.bind = "0.0.0.0"` in the mounted configuration):

```bash
docker run --rm -p 8080:8080 \
  -v "$PWD/stuntdouble.toml:/etc/stuntdouble/stuntdouble.toml:ro" \
  ghcr.io/geeknonerd/stuntdouble:v1.0.1
```

Contributors install from source with `cargo install --path .`; the full asset list and release checklist live in [docs/development.md](docs/development.md).

## Features

- Route model with one pipeline: `match → source → transform → response`.
- Built-in JavaScript runtime (Boa) with a host-injected `ctx`; no raw `fetch`, `fs`, `os`, `subprocess`, or `socket`.
- Upstream HTTP (`ctx.http.get`, streaming `ctx.http.pipe` with Range passthrough) plus `ctx.file` reads, file streaming, and request-scoped multipart uploads.
- One static file root with path traversal protection; static configuration with restart.
- Linux x86_64, macOS arm64, and Windows x86_64 binaries plus `linux/amd64` and `linux/arm64` container images.
- Structured per-request logs with `request_id`, the upstream call chain, and stable error classes.

## Non-goals for v1 (out of scope in 1.0.0)

- Request-to-request shared state; response sequencing; automatic resource CRUD.
- TypeScript transpilation; Python runtime (v2, see ADR 0015); npm, pip, or third-party imports.
- Hot reload, Admin API, GUI; built-in TLS termination.
- WebSocket, GraphQL, gRPC.
- `ctx.http.request`, `retries`, `bodyBytes` (deferred, see ADR 0016).

See [plans/product-definition.md](plans/product-definition.md) for the full scope.

## Documentation

- [Getting started](docs/guide/getting-started.md) — install, first route, and upstream calls.
- [Mock recipes](docs/guide/mock-recipes.md) — task-oriented JSON, upstream, file, upload, and CI examples.
- [Public contracts](docs/contracts/) — configuration, `ctx` API, and CLI.
- [`ctx` API type definitions](types/ctx-api-v1.d.ts) — apiVersion 1 source types.
- [Demo fixture](demo/README.md) — runnable offline and upstream document scenarios.
- [Documentation site](https://geeknonerd.github.io/stuntdouble/) — rendered contracts and demo entry points.
- [Changelog](CHANGELOG.md) — release history.

Development documentation is written in Chinese; [ADR 0013](plans/adr/0013-documentation-language-and-bilingual-structure.md) records the language policy.

- [Product definition](plans/product-definition.md) (Chinese) — v1 scope, host API, non-goals.
- [Development and release workflow](docs/development.md) (Chinese) — branch, commit, CI, versioning, and release rules.
- [Architecture decisions](plans/adr/) (Chinese) — ADR 0001–0017.
- [Domain glossary](CONTEXT.md) (Chinese) — project vocabulary.
- [Research](research/) (Chinese) — mock server landscape, runtimes, architecture, and open-source baseline.
- [Repository documentation index](docs/README.md) (Chinese) — entry point for maintainers.

## Contributing

Before proposing a feature, read the product definition and the ADRs; new capabilities must still fit the single execution model. Follow the [Code of Conduct](CODE_OF_CONDUCT.md), use the GitHub issue templates, keep customer data out of issues and fixtures, sign off with `git commit -s` (DCO), and keep commit messages in English. See [CONTRIBUTING.md](CONTRIBUTING.md) and [docs/development.md](docs/development.md).

## Security

Do not report vulnerabilities in a public issue. Use GitHub private vulnerability reporting for this repository. See [SECURITY.md](SECURITY.md). Never paste customer hostnames, tokens, headers, production logs, request bodies, or response bodies into a public issue.

## License

Dual-licensed under either of [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT); you may choose either. Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this project is dual-licensed as above, without additional terms or conditions.

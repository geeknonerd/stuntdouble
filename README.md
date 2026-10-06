# Stunt Double

**English** \| [中文](README.zh-CN.md)

> **A test double that plays the whole show.**

Stunt Double is a Rust-based mock server for integration testing against real external dependencies. It reads upstream APIs, transforms data with built-in JavaScript, returns files and binary responses, and runs with zero host runtime dependencies.

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Release](https://img.shields.io/github/v/release/geeknonerd/stuntdouble)](https://github.com/geeknonerd/stuntdouble/releases)
[![CI](https://github.com/geeknonerd/stuntdouble/actions/workflows/ci.yml/badge.svg)](https://github.com/geeknonerd/stuntdouble/actions/workflows/ci.yml)

The configuration, `ctx` API, and CLI contracts are frozen for v1. The execution model and security boundaries live in [plans/adr/](plans/adr/); details live in [docs/](docs/README.md).

## Quickstart

This quickstart uses a prebuilt release binary, so you do not need Rust/Cargo, GitHub CLI, Docker, or a source checkout. [Download and verify the binary for your platform](#install), then open a terminal in the extracted directory.

Create an empty `files/` directory and save these two files beside the binary.

`stuntdouble.toml`:

```toml
config_version = "1"

[server]
bind = "127.0.0.1"
port = 3000

[files]
root = "./files"

[[routes]]
name = "hello"
method = "GET"
path = "/hello/:name"
script = "hello.js"
```

`hello.js`:

```js
const name = ctx.request.params.name;
ctx.respond(200, { "Content-Type": "text/plain; charset=utf-8" }, "Hello, " + name + "!\n");
```

Validate the configuration and start the server from the extracted directory.

macOS or Linux:

```sh
./stuntdouble validate --config stuntdouble.toml
./stuntdouble serve --config stuntdouble.toml
```

Windows PowerShell:

```powershell
.\stuntdouble.exe validate --config .\stuntdouble.toml
.\stuntdouble.exe serve --config .\stuntdouble.toml
```

Open <http://127.0.0.1:3000/hello/world> in a browser. The response is:

```text
Hello, world!
```

Press Ctrl-C to stop the server.

Next: [Getting started](docs/guide/getting-started.md) walks through the first route in more detail; [Mock recipes](docs/guide/mock-recipes.md) covers upstream, file, and upload flows.

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

For ordinary use, download a prebuilt binary; Rust and Cargo are not required. From the [latest release](https://github.com/geeknonerd/stuntdouble/releases/latest), download both the archive and its matching `.sha256` file. Keep them in the same folder, open a terminal or PowerShell there, and verify the checksum before extracting.

| Platform | Archive | SHA-256 file |
|---|---|---|
| Linux x86_64 | [Download](https://github.com/geeknonerd/stuntdouble/releases/latest/download/stuntdouble-x86_64-unknown-linux-gnu.tar.gz) | [Download](https://github.com/geeknonerd/stuntdouble/releases/latest/download/stuntdouble-x86_64-unknown-linux-gnu.tar.gz.sha256) |
| macOS Apple Silicon | [Download](https://github.com/geeknonerd/stuntdouble/releases/latest/download/stuntdouble-aarch64-apple-darwin.tar.gz) | [Download](https://github.com/geeknonerd/stuntdouble/releases/latest/download/stuntdouble-aarch64-apple-darwin.tar.gz.sha256) |
| Windows x64 | [Download](https://github.com/geeknonerd/stuntdouble/releases/latest/download/stuntdouble-x86_64-pc-windows-msvc.zip) | [Download](https://github.com/geeknonerd/stuntdouble/releases/latest/download/stuntdouble-x86_64-pc-windows-msvc.zip.sha256) |

The Linux and macOS commands below pass only the first line of the sidecar to the checksum tool, so an extra trailing blank line does not trigger a format warning.

### Linux (x86_64)

```sh
head -n 1 stuntdouble-x86_64-unknown-linux-gnu.tar.gz.sha256 | sha256sum --check -
tar -xzf stuntdouble-x86_64-unknown-linux-gnu.tar.gz
cd stuntdouble-x86_64-unknown-linux-gnu
./stuntdouble --version
```

### macOS (Apple Silicon)

```sh
head -n 1 stuntdouble-aarch64-apple-darwin.tar.gz.sha256 | shasum -a 256 -c -
tar -xzf stuntdouble-aarch64-apple-darwin.tar.gz
cd stuntdouble-aarch64-apple-darwin
./stuntdouble --version
```

### Windows (x64, PowerShell)

```powershell
$expected = (Get-Content -Raw .\stuntdouble-x86_64-pc-windows-msvc.zip.sha256).Trim().Split(' ')[0]
$actual = (Get-FileHash .\stuntdouble-x86_64-pc-windows-msvc.zip -Algorithm SHA256).Hash
if ($actual -ne $expected) { throw "SHA-256 mismatch" }
"SHA-256 verified"
Expand-Archive -Path .\stuntdouble-x86_64-pc-windows-msvc.zip -DestinationPath .\stuntdouble-x86_64-pc-windows-msvc
Set-Location .\stuntdouble-x86_64-pc-windows-msvc
.\stuntdouble.exe --version
```

If a checksum does not match, do not run the archive; download both files again and retry. The extracted directory contains the executable, so no installer or administrator access is needed.

### Optional: verify release provenance

The SHA-256 commands check that the archive matches the checksum published with the release. They do not verify how it was built. To verify the GitHub artifact attestation and build provenance, install the [GitHub CLI (`gh`)](https://github.com/cli/cli#installation) separately, then run this with the archive you downloaded:

```sh
gh attestation verify stuntdouble-x86_64-unknown-linux-gnu.tar.gz --repo geeknonerd/stuntdouble
```

Replace the example filename with your downloaded archive. GitHub CLI is not required to install or run Stunt Double.

### Run the container image (optional)

This option requires Docker. In `stuntdouble.toml`, keep your existing `config_version` and `routes`, and edit the existing `[server]` and `[files]` tables to use these values (do not add duplicate tables):

```toml
[server]
bind = "0.0.0.0"
port = 8080

[files]
root = "./files"
```

Keep the referenced `files/` directory and route scripts in the same project directory as `stuntdouble.toml`. From a macOS or Linux shell in that directory, run:

```sh
docker run --rm -p 127.0.0.1:8080:8080 \
  -v "$PWD:/etc/stuntdouble:ro" \
  ghcr.io/geeknonerd/stuntdouble:v1.0.1
```

The image reads `/etc/stuntdouble/stuntdouble.toml` and serves on port `8080`.

### Build from source (contributors)

This path is for contributors with a repository checkout and a Rust toolchain:

```sh
cargo install --path .
```

See [docs/development.md](docs/development.md) for the full release asset list and release checklist.

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

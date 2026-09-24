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

**Script execution, upstream HTTP, request diagnostics, and sandbox hardening (T1–T7) are implemented; the v1 public contracts and `ctx` API type definitions are frozen (T8).** `stuntdouble serve` and `stuntdouble validate` run from a TOML configuration, match routes by method and path (`:param` capture), and execute each matched route's JavaScript in the embedded Boa runtime with a host-injected `ctx` (`apiVersion`, `request`, `http.get`, `http.pipe`, `file.readText`, `file.readBytes`, `file.stream`, `respond`, `log`, `env`). Both upstream calls reach only hosts listed in `[upstream] allow_hosts`: `ctx.http.get` treats upstream 4xx/5xx responses as data and maps uncaught transport failures to 502 `upstream_unreachable`, while `ctx.http.pipe` streams a 2xx body to the client around the script heap, forwards `Range`, and keeps 206 `Content-Range`. Unmatched routes answer 404 `not_found`; scripts that throw or time out answer 500 `script_error`, and scripts that never call `ctx.respond` answer 500 `script_no_response` — always with a `request_id`. Script-supplied response headers are validated fail-closed: malformed names or values answer `script_error`, and `ctx.http.pipe` rejects them before contacting the upstream. Scripts also run under a host-owned resource envelope: the configured `sandbox.script_timeout_ms` reply deadline, the `server.request_timeout_ms` deadline for reading one request head and body, a loop-iteration backstop, and pinned recursion/VM-stack limits. An engine panic answers 500 `script_error` instead of taking the server down, but Boa 0.22 exposes no heap metric or interrupt hook, so no in-process heap cap is enforced — see [SECURITY.md](SECURITY.md) for the threat model and the [ADR 0003](plans/adr/0003-script-first-multi-runtime.md) T3 amendment for the tradeoff. The document manifest and PDF download scenarios from `plans/demo-document-catalog.md` run from the in-repo fixture in [`demo/`](demo/README.md), which serves both the upstream routes and fully offline file routes. Every request also writes one structured log line to stderr with the upstream call chain, script duration, body sizes, and allowlisted headers; `serve --verbose` attaches a stable `detail` class to 500/502 responses for local diagnosis, never stack traces, upstream bodies, or internal addresses. T10 adds signal-driven shutdown: the first Ctrl-C/SIGINT or Unix SIGTERM drains in-flight requests and exits `0`; a second signal observed after the shutdown starts forces exit `130`/`143` (standard signals are not queued, so back-to-back signals may coalesce). T9 configures the release pipeline: the release contract requires `release-plz` to own version PRs and tags, cargo-dist to build the Linux x86_64, macOS arm64, and Windows x86_64 archives with checksums, attestations, and the `apiVersion` 1 type definition, and the post-release job to attach the CycloneDX SBOM and the GHCR image tag and digest. Release [`v0.2.0`](https://github.com/geeknonerd/stuntdouble/releases/tag/v0.2.0) already ships those assets — the three platform archives with per-file checksums, `SHA256SUMS`, the CycloneDX SBOM, `ctx-api-v1.d.ts`, and the GHCR image digest — and the release workflow verifies each one before announcing; [docs/development.md](docs/development.md) keeps the verification checklist. Rooted `ctx.file` reads and streamed local file responses are implemented (T11), and T12 parses multipart uploads before the script into request-scoped temporary storage exposed as `ctx.request.files`. T13 makes the document fixture runnable without any upstream: the local manifest reads `files/metadata.json` with `ctx.file.readText`, the local download streams the fixture PDF with `ctx.file.stream` and answers 200/206/416, and the upload route echoes multipart metadata without persisting anything.

The execution model and security boundaries were fixed before implementation details. See [plans/adr/](plans/adr/) and [docs/solutions/](docs/solutions/) (both in Chinese).

## Install and verify

```bash
cargo install --path .   # or: cargo run -- serve --config stuntdouble.toml

# Container image (replace the tag with one from the releases page;
# set server.bind = "0.0.0.0" in the mounted configuration)
docker run --rm -p 8080:8080 \
  -v "$PWD/stuntdouble.toml:/etc/stuntdouble/stuntdouble.toml:ro" \
  ghcr.io/geeknonerd/stuntdouble:vX.Y.Z
```

The release contract requires each [GitHub Release](https://github.com/geeknonerd/stuntdouble/releases) to include the Linux x86_64, macOS arm64, and Windows x86_64 archives, `SHA256SUMS`, the CycloneDX SBOM, `ctx-api-v1.d.ts`, release notes, and the GHCR image tag plus digest for `linux/amd64` and `linux/arm64`; the release workflow verifies that every required asset is attached. Verify the provenance of a downloaded archive:

```bash
gh attestation verify stuntdouble-x86_64-unknown-linux-gnu.tar.gz --repo geeknonerd/stuntdouble
```

The trigger chain and the release checklist live in [docs/development.md](docs/development.md).

## Planned v1

- Route model with one pipeline: `match → source → transform → response`.
- Built-in JavaScript runtime: Boa.
- Built-in Python runtime: RustPython with a stdlib subset.
- Host-injected `ctx` API. No raw `fetch`, `fs`, `os`, `subprocess`, or `socket`.
- Upstream HTTP (`ctx.http.get` plus streaming `ctx.http.pipe` with Range passthrough; `ctx.http.request` pending) and `ctx.file` reads, file streaming, local file Range responses, and request-scoped multipart uploads exposed through `ctx.request.files`.
- One static file root with path traversal protection.
- Static configuration with restart. Hot reload and Admin API are deferred.
- Linux x86_64, macOS arm64, and Windows x86_64 binaries plus a `linux/amd64` and `linux/arm64` container image (release automation configured in T9).
- Structured per-request logs with `request_id`, the upstream call chain, body sizes, allowlisted headers, and stable error classes.

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
- [Mock recipes](docs/guide/mock-recipes.md) — task-oriented JSON, upstream, file, upload, and CI examples.
- [Public contracts](docs/contracts/) — configuration, `ctx` API, and CLI.
- [`ctx` API type definitions](types/ctx-api-v1.d.ts) — apiVersion 1 source types.
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

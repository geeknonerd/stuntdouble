# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Ninth implementation slice (T9): release automation is active. `release-plz` owns version PRs,
  tags, and changelog updates (`publish = false`, `git_only = true`) and dispatches cargo-dist for
  each tag. cargo-dist builds Linux x86_64, macOS arm64, and Windows x86_64 `.tar.gz`/`.zip`
  archives with per-file checksums and `sha256.sum`, attaches `SHA256SUMS` and
  `types/ctx-api-v1.d.ts`, and creates GitHub artifact attestations. A post-announce job generates a
  CycloneDX SBOM, publishes the multi-stage container image to
  `ghcr.io/geeknonerd/stuntdouble:<tag>` with an `actions/attest-build-provenance` attestation and a
  digest asset, verifies the released Linux archive and container attestations with
  `gh attestation verify`, and asserts that every required release asset is attached. CodeQL
  advanced scanning runs on `main`, pull requests, and a weekly schedule, and the MSRV job now
  checks the locked dependency graph. Repository setup enables GitHub Actions-created pull requests;
  a fine-grained `RELEASE_PLZ_TOKEN` secret is optional and makes release-PR CI run automatically.
- Tenth implementation slice (T10): `serve` now handles Ctrl-C (SIGINT) on all platforms and
  SIGTERM on Unix. The first signal stops accepting new connections and drains in-flight requests
  through `axum::serve(...).with_graceful_shutdown(...)` before exiting `0`; a second signal observed
  after the first has started the shutdown abandons the drain and terminates immediately with exit
  code `130` (SIGINT/Ctrl-C) or `143` (SIGTERM). Standard signals are not queued, so back-to-back
  signals delivered before the first is observed may coalesce into that first signal's normal drain.
  Signal handling lives at the CLI boundary: `server::run` takes a shutdown future and only owns the
  drain. Unix end-to-end tests cover both first signals, SIGINT/SIGTERM draining, the forced
  second-signal path, and the coalesced-signal fallback. Migration: scripts that treated `130`/`143`
  as the normal stop code should accept `0`; those codes now mean the operator forced an immediate
  stop. The ADR 0011 T10 amendment records the exit-code and platform semantics.
- Eighth implementation slice (T8): the first vertical slice's public contracts are frozen.
  `docs/contracts/config.md`, `docs/contracts/cli.md`, and `docs/contracts/ctx-api.md` now document the
  complete configuration schema, the actual CLI exit codes and output, and the implemented
  `apiVersion` 1 subset; incomplete capabilities (`ctx.request.bodyBytes`, `ctx.http.request`,
  `ctx.file.*`, `ctx.local`, and timers) are marked pending explicitly. `types/ctx-api-v1.d.ts` is the
  public editor type definition for the implemented subset; the release artifact contract requires
  it, and T9 wires that pipeline. The manifest and PDF download demo scenarios remain validated by
  end-to-end tests.
- Sandbox hardening slice (T3): one script run now has an explicit resource envelope — the
  configured `sandbox.script_timeout_ms` deadline, a loop-iteration backstop, and pinned Boa
  recursion/VM-stack limits — and an engine panic is contained by the host guard and mapped to
  500 `script_error` instead of unwinding into the server task. Regression tests cover the
  deadline actually firing, runaway recursion in one route leaving other routes healthy, and the
  absence of raw `fetch`/`fs`/`process`/`require` and host bridge globals from the script realm.
  Boa 0.22 exposes no heap metric or interrupt hook, so ADR 0003 and the `ctx-api` contract's
  security boundary record that the documented ~64MB heap cap cannot be enforced in-process; the
  remaining bounds and the subprocess upgrade path are documented there.
- Seventh implementation slice (T7): every request now writes one structured log line with the
  matched route, script duration, ordered upstream call chain (`api`, `host`, `path`, `status`,
  `response_bytes`, `duration_ms`, `redirects`, `error`/`kind`), request/response body sizes, and
  allowlisted headers; request and response bodies, cookies, and authorization headers are never
  logged by the host. A streamed `ctx.http.pipe` response logs when its body ends instead of when
  headers arrive: the line carries the relayed byte count and classifies a mid-stream upstream read
  failure as `upstream_stream_error` (or `client_disconnected` when the client leaves first) while
  the already-sent 2xx status stays unchanged. `serve --verbose` now attaches a stable `detail`
  string to engine-generated 500/502 JSON error bodies (for example
  `upstream transport failure: timeout`); without the flag, error bodies keep only `error` and
  `request_id`. Stack traces, script messages, upstream bodies, hostnames, and URLs never reach
  clients in either mode, and a client `X-Request-ID` is recorded as `client_request_id` but never
  adopted or forwarded upstream. `script_logs[].message` is script-authored and unredacted, so
  `ctx.log.*` must keep bodies and secrets out, and runtime log lines are operator-facing
  diagnostics that must be redacted before being published.
- `docs/guide/getting-started.md` and its `.zh-CN.md` translation walk through the first route, `validate`/`serve`, and an allowlisted `ctx.http.get` call; the Chinese translations of the public contracts, the Pages landing page, and the demo README ship alongside it.
- Sixth implementation slice (T6): `ctx.http.pipe(url, {status, headers})` streams one allowlisted
  upstream 2xx body straight to the client without entering the script heap. The client `Range` request
  header is forwarded, an upstream 206 keeps its `Content-Range` header and status, `status` defaults
  to the upstream 2xx status; a final non-2xx answer raises `upstream_http_error`, a URL that does not
  parse or is not http/https raises `upstream_url_invalid`, and an unfollowable redirect chain raises
  `upstream_redirect_error`, so route scripts keep their own error mapping. The T6 amendment to
  ADR 0005 records why `ctx.http.pipe` cannot treat a non-2xx answer as data, and why a mid-body
  upstream failure can only truncate a stream already on the wire. The
  repository also ships the demo download route `GET /demo/documents/download/:document_id` from
  `plans/demo-document-catalog.md` §3.2: exact `code` matching, 404 `document_not_found`, 502
  `pdf_url_invalid`, 502 `pdf_bad_gateway`, and `Content-Type`/`Content-Disposition` headers. End-to-end
  tests cover the streaming path, Range passthrough, error mapping, and a body larger than the 8 MiB
  `ctx.http.get` cap.
- New dependency: `tokio-stream` 0.1 (MIT) adapts the piped body channel into the HTTP response
  stream; tokio supplies the channel, but neither the standard library nor the existing dependencies
  provide a `Stream` adapter for it.
- Fifth implementation slice (T5): the repository ships the runnable demo fixture from
  `plans/demo-document-catalog.md` §3.1 (`demo/stuntdouble.toml` plus `demo/scripts/manifest.js`).
  `GET /demo/documents/manifest/:group` reads upstream metadata through `ctx.http.get` and answers
  `text/plain; charset=utf-8` CSV with the fixed `文件编码,文件标题,系统代码` header, quoting any field
  that contains a comma, a double quote, CR, or LF; an empty `data` array answers the header row plus
  one trailing newline. Metadata responses that are non-2xx, unreadable as JSON, or missing the `data`
  array, plus metadata transport failures, answer 502 `metadata_bad_gateway`; allowlist and URL policy
  failures stay 500 `script_error`, per ADR 0005. End-to-end tests drive the built binary against the
  in-repo fixture with a stdlib fake upstream.
- Fourth implementation slice (T4): `ctx.http.get` performs allowlisted upstream HTTP GETs from route scripts. `[upstream] allow_hosts`/`timeout_ms` and per-call `opts.timeout_ms` bound access; redirects are followed manually for at most 3 hops with protocol and host re-validation on every hop. Upstream 4xx/5xx responses are data, transport failures are catchable script exceptions, and uncaught transport failures map to 502 `upstream_unreachable` with a `request_id`.
- `ctx.http.get` now caps upstream bodies at 8 MiB and carries them as base64, decoding into a `Uint8Array` only when `bytes()` is called; larger bodies will use `ctx.http.pipe` (T6). Upstream requests are direct and ignore environment proxy variables. The `apiVersion` 1 type definition source lives at `types/ctx-api-v1.d.ts` and is part of the frozen v1 contract (T8).
- New dependencies: `ureq` 3.4 (MIT OR Apache-2.0) with rustls and the platform certificate verifier for HTTPS, `rustls` 0.23 with the ring provider, `base64` 0.23 (MIT OR Apache-2.0) for the bounded binary bridge, and `url` 2 (MIT OR Apache-2.0) for URL parsing and redirect resolution; the standard library has no HTTP, TLS, or base64 codec. TLS uses the host trust store instead of a bundled root data set.
- Second implementation slice (T2): embedded Boa runtime executes each matched route script with a host-injected `ctx` (`apiVersion`, `request`, `respond`, `log`, `env`); script errors, timeouts, and missing responses map to 500 with stable error classes and a `request_id`.
- Optional `[sandbox]` configuration table with `script_timeout_ms` (default 10000, must be positive). Timeout enforcement is deadline-based with a loop-iteration backstop, because Boa 0.22 exposes no interrupt hook.
- Route scripts may return text or byte bodies through `ctx.respond`; `ctx.log.*` records stay in server logs and never reach clients.
- Dependencies refreshed on latest compatible releases: `toml` upgraded to 1.x (the loader now calls `toml::from_str`, because toml 1.x `FromStr` parses a single value rather than a document); clap, tokio, axum, and serde_json were already current.
- MSRV is Rust 1.91, set by Boa 0.22 (clap 4.6 and toml 1.x require 1.85). Boa 0.20 / 0.21 were evaluated for their lower MSRV, but their dependency chain still uses the archived `paste` crate and keeping the MSRV low would pin `time` to a version affected by RUSTSEC-2026-0009; `cargo deny check advisories` must pass, so the advisory-clean Boa line wins.
- First implementation slice (T1): Cargo project with clap CLI (`serve`/`validate`), TOML config loader with fail-closed schema, route matching (method + path/:param), HTTP 404/501 responses with request_id correlation, and per-request structured logging.
- `stuntdouble validate` command checks configuration syntax and semantics without binding sockets; reports violations with dotted field paths, expected shapes, and actual values.
- Global `-c/--config` flag usable before or after the subcommand (`stuntdouble --config x serve` and `stuntdouble serve --config x`); defaults to `stuntdouble.toml`.
- `stuntdouble serve` starts the mock server, accepts numeric bind addresses (127.0.0.1 default) and port (3000 default). Unmatched routes return 404; matched routes return 501 `script_unimplemented` pending transform slices.
- Each request generates unique request_id (non-crypto), logged per-request and included in error responses (404/501) via body and `X-Request-ID` header.
- CI gates activated for fmt, clippy (pedantic), tests, docs, deny, audit, MSRV (1.82 candidate).
- Bin+lib crate structure enables `cargo test --doc`: src/lib.rs exports config/matcher/server modules; src/main.rs imports from stuntdouble::{config,server}. See [docs/solutions/ci/doctest-lib-target-required.md](docs/solutions/ci/doctest-lib-target-required.md).

### Changed
- Documentation now follows the three-tier language model recorded in ADR 0013: community and legal documents stay English, public documents (README, Pages landing page, `docs/guide/`, `docs/contracts/`, `demo/README.md`) ship English plus a `.zh-CN.md` translation, and development documents are Chinese. A new `docs-links` CI gate checks local Markdown links and translation pairs.
- Dependency license and advisory checks now cover the shipped platforms (Linux x86_64, macOS arm64, Windows x86_64) instead of every target in the lockfile; `rustls-platform-verifier` carries Android/wasm-only root bundles whose data license is outside the project allowlist.
- The CLI contract now documents the final shutdown behavior: the first SIGINT/SIGTERM drains in-flight requests and returns `0`, while a second signal forces an immediate `130`/`143` exit. The T8/T9 amendment to ADR 0012 records that `.d.ts` publication is a release-blocking T9 acceptance item.
- Contract documents are frozen for the v1 slice (T1–T8): `docs/contracts/config.md` includes the complete configuration and Route schema, `docs/contracts/cli.md` documents commands, flags, output, and exit codes, and `docs/contracts/ctx-api.md` marks the implemented `apiVersion` 1 subset and pending capabilities. `types/ctx-api-v1.d.ts` is the matching public type definition.

- Open-source repository baseline: README, contribution guide, security policy, code of conduct, issue templates, and pull request template.
- Dual license: MIT OR Apache-2.0.
- GitHub Pages landing page.
- Git workflow, versioning, release, compatibility, and supply-chain decision records.
- Initial draft contracts for configuration, `ctx` API, and CLI.
- Governance and development guides.
- CI, PR title, and DCO workflows.
- Dependabot configuration for Cargo and GitHub Actions.
- Repository labels for triage, area, status, and security.

### Fixed

- A fully delivered `ctx.http.pipe` response is no longer logged as `client_disconnected` when hyper
  closes the relay as soon as the announced `Content-Length` is satisfied. The completion line keeps
  `"error": ""` once the relayed bytes reach that length, and `client_disconnected` stays reserved
  for a client that leaves before the body ends.
- Script-supplied response headers are validated when `ctx.respond` or `ctx.http.pipe` is called and
  again before the Response is constructed. Malformed header names or values now return 500
  `script_error` instead of being silently dropped; `ctx.http.pipe` fails before contacting the
  upstream.
- `stuntdouble validate` now rejects unknown `config_version` values, non-IP `server.bind` values,
  and an empty `routes` array with exit code `2`, and it reports `routes` and `files.root`
  violations together, matching the frozen v1 configuration contract.

# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Seventh implementation slice (T7): every request now writes one structured log line with the
  matched route, script duration, ordered upstream call chain (`api`, `host`, `path`, `status`,
  `duration_ms`, `redirects`, `error`/`kind`), request/response body sizes, and allowlisted headers;
  request and response bodies, cookies, and authorization headers are never logged. `serve --verbose`
  now attaches a stable `detail` string to 500/502 JSON error bodies (for example
  `upstream transport failure: timeout`); without the flag, error bodies keep only `error` and
  `request_id`. Stack traces, script messages, upstream bodies, hostnames, and URLs never reach
  clients in either mode, and a client `X-Request-ID` is recorded as `client_request_id` but never
  adopted or forwarded upstream.
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
- `ctx.http.get` now caps upstream bodies at 8 MiB and carries them as base64, decoding into a `Uint8Array` only when `bytes()` is called; larger bodies will use `ctx.http.pipe` (T6). Upstream requests are direct and ignore environment proxy variables. The `apiVersion` 1 type definition source was added at `types/ctx-api-v1.d.ts`; T8 (#11) publishes it with release artifacts.
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

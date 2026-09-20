---
title: Bundled webpki roots fall outside the MIT/Apache license allowlist
date: 2026-09-21
category: dependencies
module: upstream HTTP TLS trust
problem_type: license_policy_conflict
component: upstream
symptoms:
  - "cargo deny check licenses failed on webpki-roots 1.0.9 with CDLA-Permissive-2.0"
  - "The HTTPS client compiled, but the repository license gate rejected the bundled root store"
root_cause: bundled_root_store_license
resolution_type: dependency_swap
severity: medium
tags: [tls, cargo-deny, licenses, ureq, rustls]
---

# Bundled webpki roots fall outside the MIT/Apache license allowlist

## Problem

`ctx.http.get` needs HTTPS support, and the first `ureq` configuration used its
default `rustls` feature. That feature pulls `webpki-roots` 1.0.9, whose data is
licensed under CDLA-Permissive-2.0. The repository's dependency policy allows
MIT OR Apache-2.0, so `cargo deny check licenses` rejected the crate even though
the Rust code itself is permissively licensed.

## What Didn't Work

- **Adding CDLA-Permissive-2.0 to the allowlist**: `AGENTS.md` requires new
  dependencies to be MIT OR Apache-2.0; broadening the allowlist for a bundled
  data dependency would silently weaken that rule.
- **Keeping `ureq`'s default `rustls` feature**: the rest of the TLS stack is
  fine, but the bundled root store stays in the dependency graph.

## Solution

Use `ureq` with `rustls-no-provider` plus `rustls-platform-verifier`, and supply
the ring crypto provider explicitly:

- `ureq = { default-features = false, features = ["rustls-no-provider", "platform-verifier"] }`
- `rustls = { default-features = false, features = ["ring"] }`
- The agent's `TlsConfig` sets `RootCerts::PlatformVerifier` and the ring provider.

TLS trust now comes from the host store, so enterprise trust roots installed in
the OS also work. `rustls-platform-verifier` still mentions
`webpki-root-certs` for wasm/android targets; `deny.toml` therefore scopes
`cargo-deny` to the shipped platforms (Linux x86_64, macOS arm64, Windows
x86_64) via `[graph] targets`, and `docs/development.md` documents that scope.

## Verification

- `cargo deny check` passes for licenses, sources, bans, and advisories.
- `cargo audit` reports no advisories for the locked graph.
- `ctx.http.get` end-to-end tests cover 2xx, 4xx/5xx, redirects, timeouts, and
  transport failures.

## Tradeoffs

- TLS certificate trust is delegated to the operating system instead of a
  bundled Mozilla root set. This is the desired behavior for a local mock
  server and makes enterprise MITM roots usable.
- Dependency checks cover the platforms the project actually ships;
  target-specific dependencies of unsupported platforms are not part of the gate.

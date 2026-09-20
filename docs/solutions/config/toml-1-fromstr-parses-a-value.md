---
title: toml 1.x `FromStr` parses a value, not a document
date: 2026-09-20
category: config
module: TOML configuration parsing
problem_type: api_behavior_change
component: config
symptoms:
  - "After upgrading to toml 1.x every configuration failed with `TOML parse error at line 1, column 15 / unexpected content, expected nothing`"
  - "The same fixture parsed fine with toml 0.8, and the error pointed at the first `key = value` pair, not at a syntax mistake"
root_cause: api_semantics_change
resolution_type: code_fix
severity: high
tags: [toml, dependency-upgrade, parsing, config]
---

# toml 1.x `FromStr` parses a value, not a document

## Problem

`stuntdouble validate` rejected every configuration after `toml` moved from 0.8 to 1.x, including the repository's own fixtures. The parser reported:

```text
TOML parse error at line 1, column 15
  |
1 | config_version = "1"
  |               ^
unexpected content, expected nothing
```

The error is misleading: the input is valid TOML. The loader was using `text.parse::<toml::Value>()`.

## What Didn't Work

- **Downgrading within 1.x**: `toml 1.0.7` reproduced the failure, so this is not a regression in a single patch release.
- **Lowering the MSRV stack to keep toml 0.8**: Boa 0.20 / 0.21 drag in the archived `paste` crate and require a `time` version affected by RUSTSEC-2026-0009, so `cargo deny check advisories` fails. This had to be fixed in code, not by pinning old dependencies.

## Solution

Parse documents with `toml::from_str`, which is the document-level entry point in both 0.8 and 1.x:

```rust
// Before: parses a single inline value as of toml 1.x
let root: toml::Value = text.parse().map_err(/* ... */)?;

// After: parses the whole document
let root: toml::Value = toml::from_str(&text).map_err(/* ... */)?;
```

`src/config.rs` carries a comment pointing at this difference so the next reader does not re-learn it.

## Why This Works

In `toml` 1.x, `impl FromStr for Value` delegates to `ValueDeserializer::parse` — an inline-value parser — so a document such as `config_version = "1"` fails after the first token. `toml::from_str::<toml::Value>` uses the document deserializer instead and returns the expected table. `toml 0.8` happened to accept both, which is why the upgrade surfaced the latent misuse.

## Prevention

- Treat `str::parse::<toml::Value>()` as "parse one value"; use `toml::from_str` whenever the input is a configuration document.
- Keep at least one end-to-end `validate --config` test with a real file so parser-semantics changes fail loudly instead of only in unit fixtures.
- When a dependency major version lands, run the full gate set (`test`, `clippy`, `deny`) before touching MSRV numbers; the first failure may be a semantic change rather than a version constraint.

## Related Issues

- `Cargo.toml` now depends on `toml = "1"`.
- MSRV/security trade-off between Boa 0.20, 0.21, and 0.22 is recorded in [ADR 0003](../../../plans/adr/0003-script-first-multi-runtime.md) and `docs/development.md`.

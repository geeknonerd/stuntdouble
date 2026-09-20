---
title: Cargo doctests require a library target
date: 2026-09-20
category: ci
module: Rust build & testing
problem_type: build_error
component: ci
symptoms:
  - "GitHub Actions docs job failed with: error: no library targets found in package `stuntdouble`"
  - "Local command `cargo test --doc` returned same error; all other gates (fmt/clippy/test) passed"
root_cause: missing_tooling
resolution_type: tooling_addition
severity: high
tags: [doctest, cargo, library-target, ci]
---

# Cargo doctests require a library target

## Problem

GitHub Actions CI job `docs` runs `cargo test --doc`, which requires at least one `[lib]` target in `Cargo.toml`. The stuntdouble crate only defined `[[bin]]` (src/main.rs), so doctests fail with "no library targets found". This blocks merge until CI passes.

## Symptoms

- **CI failure**: `cargo test --doc` in `.github/workflows/ci.yml` step `"Run cargo test --doc"` concludes **failure**
- **Error text**: `error: no library targets found in package 'stuntdouble'`
- **Local repro**: Running `cargo test --doc` locally produces identical error; `cargo test` alone passes because it only tests bins/tests
- **Gates pass**: All other required checks (`fmt`, `clippy`, `test`) succeed; only `docs` gate is blocked

```bash
$ cargo test --doc
error: no library targets found in package `stuntdouble`
```

## What Didn't Work

- **Remove doctest gate from CI**: This would violate project requirements (`docs/development.md` mandates `cargo test --doc`).
- **Switch to external docs tools**: Replacing Rust doctests with mdbook or similar adds complexity and breaks existing workflow expectations.

## Solution

1. **Create `src/lib.rs`** to expose modules as public library API:
   ```rust
   pub mod config;
   pub mod matcher;
   pub mod server;
   ```
2. **Refactor `src/main.rs`** to be CLI-only entry point that imports from `stuntdouble::{config, server}`:
   ```rust
   use stuntdouble::{config, server};
   ```
3. **Update Cargo.toml** implicitly by ensuring both `[[bin]]` (existing) and library target (via lib.rs) exist.
4. **Add `#[must_use]` attributes** to functions returning `Option` used for error signaling (`normalize_method`, `match_route`) to satisfy clippy pedantic lint.

All gates then pass locally:

```bash
cargo fmt --all               # ✅ pass
cargo clippy -- -D warnings    # ✅ No issues found
cargo test                    # ✅ 17 passed
cargo test --doc              # ✅ 0 passed (no errors)
```

Commit `6ffb80a fix: split src/main.rs into src/lib.rs + src/main.rs to enable cargo test --doc`.

## Why This Works

Rust's `cargo test --doc` runs doctests in **library targets**. A pure-bin crate (`[[bin]]` only) has no library code for doctests, resulting in `no library targets found`. By introducing `src/lib.rs`:

- Cargo detects both a library and binary target
- `main.rs` can re-export library components for CLI usage while keeping module definitions centralized in `lib.rs`
- Doctests can run against public types/functions declared in the library

This pattern (bin+lib separation) is standard for CLI tools and aligns with `cargo doc` also requiring a library target for documentation generation.

## Prevention

- **Always define a library target when you expect doctests**: If your CI workflow includes `cargo test --doc` or `cargo doc --no-deps`, ensure `Cargo.toml` has a `[lib]` section (implicit via presence of `src/lib.rs`).
- **Check CI before pushing**: For projects where CI gates are mandatory (like stuntdouble), run all gates locally before creating PRs. Specifically include `cargo test --doc` even if it seems redundant.
- **Document build conventions**: Add a note in `CONTRIBUTING.md` or `docs/development.md` stating that Rust crates for CLI tools should use `bin+lib` structure when doctests are enabled.
- **Pre-commit hooks**: Consider adding scripts that validate `Cargo.toml` contains a library target when `.github/workflows/ci.yml` references `cargo test --doc`.

## Related Issues

- GitHub PR #13 resolved this issue with commit `6ffb80a` (merge confirmed green).
- See `docs/contracts/cli.md` for CLI contract documentation; updated in the fix.
- See `docs/development.md` for CI requirements including `cargo test --doc` gate.

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

# Cargo doctest 需要 library target

## 问题

GitHub Actions 的 `docs` job 运行 `cargo test --doc`，它要求 `Cargo.toml` 中至少有一个 `[lib]` target。stuntdouble crate 当时只定义了 `[[bin]]`（src/main.rs），因此 doctest 报 "no library targets found"，在 CI 通过前阻塞合并。

## 症状

- **CI 失败**：`.github/workflows/ci.yml` 中 `"Run cargo test --doc"` 步骤的 `cargo test --doc` 结论为 **failure**
- **错误文本**：`error: no library targets found in package 'stuntdouble'`
- **本地复现**：本地运行 `cargo test --doc` 得到相同错误；单独运行 `cargo test` 通过，因为它只测试 bin 与 tests
- **其余门禁通过**：所有其他必需检查（`fmt`、`clippy`、`test`）都成功，只有 `docs` 门禁被阻塞

```bash
$ cargo test --doc
error: no library targets found in package `stuntdouble`
```

## 试过但无效的做法

- **从 CI 移除 doctest 门禁**：违反项目要求（`docs/development.md` 规定必须运行 `cargo test --doc`）。
- **改用外部文档工具**：用 mdbook 之类替换 Rust doctest 会增加复杂度，并偏离既有工作流预期。

## 解决方案

1. **创建 `src/lib.rs`**，把模块暴露为公共 library API：
   ```rust
   pub mod config;
   pub mod matcher;
   pub mod server;
   ```
2. **重构 `src/main.rs`**，使其只作 CLI 入口，从 `stuntdouble::{config, server}` 导入：
   ```rust
   use stuntdouble::{config, server};
   ```
3. **更新 Cargo.toml**：确保 `[[bin]]`（原有）与 library target（由 lib.rs 隐式提供）同时存在。
4. **给用于错误信号的 `Option` 返回函数加 `#[must_use]`**（`normalize_method`、`match_route`），以满足 clippy 的 pedantic lint。

之后本地全部门禁通过：

```bash
cargo fmt --all               # ✅ pass
cargo clippy -- -D warnings    # ✅ No issues found
cargo test                    # ✅ 17 passed
cargo test --doc              # ✅ 0 passed (no errors)
```

对应修复：PR #13（squash 合并为 `ea180c1`，其中包含把 `src/main.rs` 拆成 `src/lib.rs` + `src/main.rs` 以启用 `cargo test --doc` 的改动）。

## 为什么这样可行

Rust 的 `cargo test --doc` 在 **library target** 中运行 doctest。纯 bin crate（只有 `[[bin]]`）没有可供 doctest 使用的 library 代码，于是报 `no library targets found`。引入 `src/lib.rs` 之后：

- Cargo 同时识别出 library 与 binary target
- `main.rs` 可以为 CLI 复用 library 组件，而模块定义集中在 `lib.rs`
- doctest 可以针对 library 中声明的公共类型与函数运行

这种 bin+lib 分离是 CLI 工具的标准做法，也与 `cargo doc` 同样需要 library target 的事实一致。

## 预防

- **只要依赖 doctest，就先定义 library target**：CI 若包含 `cargo test --doc` 或 `cargo doc --no-deps`，确保 `Cargo.toml` 有 `[lib]` 段（存在 `src/lib.rs` 即可隐式满足）。
- **推送前先跑 CI**：对 CI 门禁强制的项目（如 stuntdouble），建 PR 前在本地跑完全部门禁，即使 `cargo test --doc` 看起来多余也要跑。
- **把构建约定写进文档**：在 `CONTRIBUTING.md` 或 `docs/development.md` 里说明：启用 doctest 的 Rust CLI crate 应采用 bin+lib 结构。
- **pre-commit hook**：可以考虑加脚本，在 `.github/workflows/ci.yml` 引用 `cargo test --doc` 时校验 `Cargo.toml` 含 library target。

## 相关

- GitHub PR #13（squash 合并为 `ea180c1`）解决了该问题（合并确认变绿）。
- CLI 契约文档见 `docs/contracts/cli.md`，在本次修复中同步更新。
- CI 要求（含 `cargo test --doc` 门禁）见 `docs/development.md`。

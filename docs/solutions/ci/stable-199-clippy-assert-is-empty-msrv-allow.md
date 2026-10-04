---
title: "Keep assert!(x.is_empty()) under new pedantic clippy::assert_is_empty with an MSRV-compatible function-level allow"
date: 2026-10-04
category: ci
module: clippy pedantic lint compatibility
problem_type: tooling_decision
component: ci
severity: medium
applies_when:
  - "CI 的 stable 工具链新增了本地工具链还不认识的 pedantic clippy lint"
  - "被 deny 的 lint 打在有意保留的可读测试断言上，例如 assert!(x.is_empty())"
  - "仓库要守住 MSRV（rust-version），同时 CI 跟随 rust-toolchain.toml 的 stable"
tags: [clippy, pedantic, toolchain-drift, msrv, stable, ci]
---

# 用函数级 allow 接住新增 pedantic lint，并兼容 MSRV

## 背景（Context）

PR #125 把上游与文件的每请求调用链统一进 `src/request_log.rs`（`Chain<T>` / `LogRecord` / `Pending<T>`）。重构推上去之后，CI 的 `clippy` job 失败，本地工具链却一直是绿的。CI 跟随 `rust-toolchain.toml` 的 `stable`（浮动 channel，`rust-toolchain.toml:1-3`），当时已走到 Rust 1.99；本地 checkout 的 `rustc` 还是 1.98。新 stable 在 pedantic 组下新增了 `clippy::assert_is_empty`，而仓库在 `Cargo.toml:35-37` 对 `pedantic` 设了 `deny`。四处可读性测试断言（`assert!(x.is_empty())` 形态）只在 CI 上变成硬错误，本地无法复现（session history）。

当前树上的四个落点（每处一个函数级 allow，无其他改动）：`src/config.rs:857`、`src/request_log.rs:255`、`src/script.rs:1925`、`tests/cli.rs:2804`，分别守住 `assert!(config.upstream.allow_hosts.is_empty())`（`src/config.rs:862`）、`assert!(chain.snapshot().is_empty())`（`src/request_log.rs:259`）、`assert!(parse_query("").is_empty())`（`src/script.rs:1931`）、`assert!(responses[2].body_bytes.is_empty())`（`tests/cli.rs:2874`）。

## 做法（Guidance）

1. **保留可读的 `assert!(x.is_empty())` 写法，不改写成 `assert_eq!(x.len(), 0)`。** 前者是意图直述，后者只是为了躲 lint 而损失可读性，且四处都没有行为收益。
2. **每个被 deny 的测试函数上加一行函数级 allow，不做 crate 级 allow：**

   ```rust
   #[test]
   #[allow(unknown_lints, clippy::assert_is_empty)]
   fn upstream_defaults_deny_every_host_with_a_fifteen_second_timeout() {
       // ...
       assert!(config.upstream.allow_hosts.is_empty());
   }
   ```

   同一两行模式逐个落到四个站点：`src/config.rs`、`src/request_log.rs`、`src/script.rs`、`tests/cli.rs`，每函数一行 allow，不动其他代码。
3. **`unknown_lints` 必须写在前面。** 老工具链（本地 1.98、MSRV 1.91）还不认识这个 lint 名，没有 `unknown_lints` 会先报 unknown-lint 警告，又被 CI 的 `cargo clippy --all-targets --all-features -- -D warnings`（`.github/workflows/ci.yml:41`）抬成失败。先裸写 `#[allow(clippy::assert_is_empty)]` 只修了 CI、弄坏了老 clippy，就是这么来的（session history）。

## 为什么重要（Why This Matters）

- **`pedantic = deny` 加浮动 `stable`，等于每个新 pedantic lint 都是一次未来的 CI-only 中断。** 函数级 allow 把这次摩擦钉在出事的站点上：新 clippy 看到的是“已知 lint 被允许”，老 clippy 看到的是“未知 lint 被容忍”，两边都过。
- **可读断言值得保留。** `assert!(x.is_empty())` 比 `assert_eq!(x.len(), 0)` 更接近意图；crate 级 allow 则会把这次和未来的 pedantic 信号一起藏掉，是全仓范围的遮掩。
- **MSRV 约束（`Cargo.toml:5` 的 `rust-version = "1.91"`）要求写法向后兼容。** `unknown_lints` 前置是已知最便宜的双向兼容手段，不引入新依赖、不改断言语义。

## 何时适用（When to Apply）

- CI 的 `clippy` job 挂在一个本地 `cargo clippy` 不认识的 lint 上，且两边 `rustc --version` 对不上时。
- 被 deny 的 lint 打在有意为之的可读测试惯用法上，而不是真实缺陷时。
- 仓库要守住 `rust-version` 的 MSRV，同时 CI 继续跟踪 stable 时。

## 示例（Examples）

改前（stable 1.99 的 CI 上失败，本地 1.98 上通过）：

```rust
#[test]
fn out_of_range_updates_are_noops() {
    let chain: Chain<Record> = Chain::new();
    chain.update(usize::MAX, |_| panic!("must not run"));
    assert!(chain.snapshot().is_empty());
}
```

改后（两边都过，PR #125 落地形态，`src/request_log.rs:254-259`）：

```rust
#[test]
#[allow(unknown_lints, clippy::assert_is_empty)]
fn out_of_range_updates_are_noops() {
    let chain: Chain<Record> = Chain::new();
    chain.update(usize::MAX, |_| panic!("must not run"));
    assert!(chain.snapshot().is_empty());
}
```

同样的前后对照适用于 `src/config.rs:857-862` 的 `allow_hosts.is_empty()`、`src/script.rs:1925-1931` 的 `parse_query("").is_empty()`，以及 `tests/cli.rs:2804` / `tests/cli.rs:2874` 的 `body_bytes.is_empty()`。

## 相关（Related）

- [Cargo doctests require a library target](doctest-lib-target-required.md) — 同一 CI/clippy pedantic 压力面：那篇是加 `#[must_use]` 过 pedantic，本篇是函数级 allow 过新 lint；lint 不同、修法不同，`deny pedantic` 的上下文相同。
- [toml 1.x `FromStr` parses a value, not a document](../config/toml-1-fromstr-parses-a-value.md) — 同一工具链/依赖漂移家族：那篇是升级语义与 MSRV/deny 取舍（toml 1.x、Boa、安全公告），本篇是 clippy/lint 侧的漂移。
- [Cross-compile the linux/arm64 release container image on an amd64-only runner](cross-compile-arm64-container-image-on-amd64-runner.md) — 同一 `rust-toolchain.toml` stable 机制面（channel 固定与工具链选择意外），那篇是构建镜像 concern，本篇是 lint allow concern。
- PR #125（`src/request_log.rs` 调用链统一）是这次修复的载体；`docs/development.md:122` 的 CI stable 主门禁 + MSRV job 双轨是这次漂移的制度背景。

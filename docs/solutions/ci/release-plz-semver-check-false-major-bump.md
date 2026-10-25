---
title: "release-plz cargo-semver-checks causes false major bumps in unpublished binary crates"
date: 2026-10-25
category: ci
module: release-plz
problem_type: false_positive_semver_bump
component: ci
symptoms:
  - "release-plz 自动创建的发布 PR 错误地将版本由 1.3.0 提议升级为 2.0.0"
  - "PR 描述中包含 API breaking changes 告警：constructible_struct_adds_field (RequestSnapshot.raw_query / RequestSnapshot.body_bytes)"
  - "与 ADR 0011 及路线图规划冲突（M1 属于 v1.x 系列，次版本应为 1.4.0）"
root_cause: config_gap
resolution_type: configuration_change
severity: medium
tags: [release-plz, semver, cargo-semver-checks, breaking-changes, versioning]
---

# release-plz 对非公开 Rust 库运行 semver_check 导致误判 major bump

## 问题

在 #194 为请求快照增加 `raw_query` 和 `body_bytes` 字段后，GitHub Actions 的 release-plz 自动打开的发布 PR（#195）提议跳升至 `2.0.0`。

## 根因

1. **bin + lib 结构导致默认启用 semver 检查**：为支持 `cargo test --doc`，仓库根目录下配置了 `src/lib.rs`。`release-plz` 默认对包含 Rust library 的 package 调用 `cargo-semver-checks`。
2. **struct 增加公开字段被判定为 break**：`RequestSnapshot` 是 `pub struct` 且未标记 `#[non_exhaustive]`，为其实例新增公共字段破坏了下游字面量构造可能，`cargo-semver-checks` 判定其为 Rust API breaking change（`constructible_struct_adds_field`）。
3. **与仓库契约冲突**：
   - 仓库 `Cargo.toml` 标记了 `publish = false`，不发布到 crates.io，Rust 代码不作为外部库提供；
   - 根据 ADR 0011，Stunt Double 的公开契约仅包括 **CLI、配置、`ctx` API** 三个表面，内部 Rust struct 不属于公开契约；
   - 产品路线图 `plans/post-v1-roadmap.md` 将 M1 定义为 v1.x 通用底座，只有 M2（跨请求共享状态）才是 v2 的版本分界。

## 解决方案

在 `release-plz.toml` 的 `[workspace]` 中显式配置 `semver_check = false`：

```toml
[workspace]
publish = false
# Rust library semver checks are disabled because the crate is publish = false
# and its public contracts are CLI, config, and ctx API (ADR 0011). Internal
# struct changes must not trigger false major version bumps.
semver_check = false
```

这样 release-plz 将不再执行 `cargo-semver-checks`，而是纯粹依据 Conventional Commits 规范（`feat` -> minor，`fix` -> patch，`BREAKING CHANGE` / `!` -> major）计算语义版本，将本次版本正确提升为 `1.4.0`。

## 相关

- [ADR 0011](../../../plans/adr/0011-contract-compatibility.md) — 契约兼容性与演进策略
- 发布 PR #195
- release-plz 官方配置文档: `https://release-plz.dev/docs/config`

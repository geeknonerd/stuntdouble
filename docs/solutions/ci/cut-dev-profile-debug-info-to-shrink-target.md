---
title: "Cut dev profile debug info to shrink target"
date: 2026-10-05
category: ci
module: cargo dev profile target size
problem_type: tooling_decision
component: ci
severity: medium
applies_when:
  - "dev 下 target/ 膨胀到数十 GB，且大头在 debug/deps 与 debug/incremental 时"
  - "想保留本地 crate 的行号级调试，同时去掉依赖的完整调试信息时"
  - "单包项目准备加 [profile.dev] 分层调试配置，且不碰 release/dist 输出时"
  - "清理后能接受一次全量重建来验证编译与测试时"
tags: [cargo, dev-profile, debug-info, target-size, disk-usage, build-cache]
---

# 用 dev profile 分层调试信息把 target/ 从 23G 降到 2.1G

## Context

`stuntdouble` 是单包 Rust 项目，默认 dev profile 会给所有 crate（含第三方依赖）生成完整调试信息。
`target/` 因此涨到约 23G：`debug/deps` 约 14G、`debug/incremental` 约 7.8G、`debug/build` 约 316M，
其中还含有长期多 feature 变体堆积。根分区吃紧时，这堆 dev 产物是第一个该动的对象。

首版改动直接落在 `main` 上且带中文注释与悬空引用，经 code-review 后转到短分支重做，
经 PR #129 squash 合并，全绿后才落地（session history）。

## Guidance

只改 `Cargo.toml`，加 8 行（`Cargo.toml:44-50`），其余不动：

```toml
[profile.dev]
# Keep line-level debug info for local crates; drop type/generic/macro debug sections.
debug = 1

[profile.dev.package."*"]
# Dependencies skip debug info: the main driver of test binary size.
debug = false
```

要点：

- 本地 crate 用 `debug = 1`：保留行号级调试，本地仍可断点；不要直接 `debug = false`，否则连本地行号也丢了。
- 依赖用 `[profile.dev.package."*"] debug = false`：依赖不再生成调试信息，这是测试二进制体积的主要来源。
- `package."*"` 不覆盖 workspace 本地 crate：本次实证为本地测试二进制保留 `.debug_info`/`.debug_line`，
  而依赖 rlib（如 `libboa_engine-*.rlib`）无 `debug_info`（session history）。
- `[profile.dist]` 及其注释保持紧贴、不碰 release 输出；注释用英文，与仓内既有英文注释惯例一致（session history）。
- 清理与验证固定三步：`cargo clean --profile dev` 只清 dev（release 867M 保留），
  接着 `cargo test --no-run` 全量重建（本次约 1m24s），最后 `cargo test --lib` 跑通（本次 39 passed）。

暂缓项：`incremental = false` 与统一 feature 本次不做；触发条件是后续 `incremental` 或 feature 变体再明显膨胀时再启用（session history）。

## Why This Matters

默认 dev profile 把依赖的完整调试信息也全量落盘，测试目标越多、依赖越重（如 Boa），`debug/deps` 与
`debug/incremental` 胀得越快。分层后本地仍可调试、依赖不再交“调试税”，一次清理就能收回十 GB 级磁盘，
且改动可逆、只影响本地构建缓存，不碰发布产物与公开契约。

## When to Apply

- `du -sh target` 发现 dev 产物数 GB 起步，且 `debug/deps`、`debug/incremental` 占大头时。
- 本地调试只需要“能断点到本地行号”，不需要单步进依赖源码时。
- 能接受一次 `cargo clean --profile dev` 后的全量重建（本仓库约 1 分半）时。
- 单包、只有 `[profile.dist]` 这类简单 profile 布局时；workspace 多包沿用前先确认成员是否需要逐包覆盖。

## Examples

本次实测（新鲜全量重建后，`cargo test --no-run` + 文档/测试跑完后的值）：

| 项 | 优化前 | 优化后 |
|---|---:|---:|
| `target` 总量 | 约 23G | 约 2.1G |
| `debug/deps` | 约 14G | 约 1.0G |
| `debug/incremental` | 约 7.8G | 约 195M |
| `debug/build` | 约 316M | 约 38M |
| `release` | 约 867M | 约 867M（未动） |

单测试二进制从约 173M（debug 段约 117M）降到约 6.9–55M。优化前的 23G 另含长期多 feature 变体堆积，
本次随 `cargo clean --profile dev` 一并清零。

反面例子：首版两行中文注释违反仓内英文权威惯例、`disk-cleanup 附录 D` 在仓内无出处、
改动滞留 `main` 未走短分支，均在评审中被拦下并在 PR #129 前修好（session history）。
不要把这类构建调优写进 `docs/development.md` 或 `CHANGELOG.md`：本地检查命令未变，契约未变，加体积说明属于噪音。

## Related

- [Cargo doctests require a library target](../ci/doctest-lib-target-required.md)：同一 `Cargo.toml` 构建区学习，讲 library target 与 doctest 门槛；本条讲 dev profile 调试信息裁剪。
- [Cross-compile the linux/arm64 release container image on an amd64-only runner](../ci/cross-compile-arm64-container-image-on-amd64-runner.md)：同属构建区，讲 release 产物路径；本条讲 dev 产物 `target/` 体积。
- 落地：PR #129（`chore: cut dev profile debug info to shrink target`，已合入 `main`）；配置锚点 `Cargo.toml:44-50`。

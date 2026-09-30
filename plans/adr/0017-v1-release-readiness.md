# v1 发布就绪：三项待定推迟与 1.0 门禁

- 状态：`已接受`
- 日期：2026-09-30
- 关联：[ADR 0010](0010-git-and-release-workflow.md)、[ADR 0011](0011-contract-compatibility.md)、[ADR 0012](0012-release-artifacts-and-supply-chain.md)、[ADR 0015](0015-python-runtime-deferred-to-v2.md)、[ADR 0016](0016-v1-ctx-pending-capabilities-deferred.md)、[产品功能定义](../product-definition.md)、[开发指南](../../docs/development.md)

## 背景

`plans/product-definition.md` §6 仍有三项待定：`crates.io` 包名保留与首个发布凭据配置、独立治理邮箱、自定义域名。它们都不影响执行模型与三契约稳定，但不落定就无法宣布 1.0 就绪。

现状核实（2026-09-30）：`crates.io` 查询 `stuntdouble` 返回 `crate does not exist`（包名当前可用），`release-plz.toml` 为 `publish = false` + `git_only = true`，分发已由三平台二进制与容器覆盖；治理面为单维护者模型，`SECURITY.md` 与 Code of Conduct 均走 GitHub 私密报告；站点为 GitHub Pages 默认域名、无 CNAME。版本侧最新为 `v0.5.3`，`T1–T13` 已实现、`T8` 已冻结三契约，满足 ADR 0011 对 `1.0.0` 的稳定要求。

## 决策

1. **三项全部移出 1.0 门禁，列入 1.0 后事项。**
   - `crates.io`：1.0 前只做包名占位，不开自动化发布（保持 `publish = false` + `git_only = true`）。转正触发条件：出现 `cargo install` 真实需求或包名被占风险迫近；届时按 `docs/development.md` 改 `publish = true` 并配置凭据，坏版只用 `cargo yank`。
   - 治理邮箱：继续用 GitHub 私密报告，不设独立邮箱。转正触发条件：第二位维护者加入（`GOVERNANCE.md` 触发审批与 release team 变更时）或出现需脱离 GitHub 的治理事件。
   - 自定义域名：继续用 GitHub Pages 默认域名，不配 CNAME。转正触发条件：品牌或文档站点有独立域名需求；届时只改站点配置，不动发布流水线。
2. **1.0 门禁收敛为四项：** 三契约稳定（ADR 0011，T1–T12 子集见 ADR 0016）、单运行时 Boa JS（Python 已判 v2，见 ADR 0015）、发布验证链通过（draft 原子发布、三平台产物、SBOM、GHCR 双架构，见 ADR 0012）、文档与安全表修正（B 层译本核对见 ADR 0013；`SECURITY.md` 支持版本表改为 1.0 线）。
3. **版本路径直接备 `1.0.0`，不加中间 `0.6.0`。** `release-plz` 不会自动从 `0.x` 跳 `1.0`，需人工桥接（同 `0.2.0` 做法）：人工 PR 写好 `Cargo.toml 1.0.0` 与 `CHANGELOG` 段，合并后建 `v1.0.0` tag 触发产线，此后恢复版本 PR。

## 后果

**正面**：1.0 门禁可兑现；三项待定有明确触发条件，不再悬置。

**负面（接受）**：1.0 首发不支持 `cargo install stuntdouble`、无独立治理邮箱、无自定义域名。

## 替代方案

- 三项随 1.0 落地（拒绝：包名占位之外的自动化发布、邮箱与域名都是品牌与运营事项，不阻塞三契约稳定）。
- 加 `0.6.0` 过渡（拒绝：`0.5.x` 已验证发布链，无待定 breaking，加过渡只增加等待）。

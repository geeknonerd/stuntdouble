# v1 未实现 `ctx` 能力移出 1.0

- 状态：`已接受`
- 日期：2026-09-30
- 关联：[ADR 0001](0001-no-shared-state-in-v1.md)、[ADR 0004](0004-host-functions-only-sandbox.md)、[ADR 0005](0005-upstream-failure-semantics.md)、[ADR 0011](0011-contract-compatibility.md)、[ADR 0014](0014-process-isolated-script-runner.md)、[ADR 0015](0015-python-runtime-deferred-to-v2.md)、[产品功能定义](../product-definition.md)、[`ctx` API 契约](../../docs/contracts/ctx-api.md)

## 背景

`plans/product-definition.md` 自称“功能已收敛”，但 §4 表格仍把 5 组未实现能力写进 v1 口径（`bodyBytes`、`ctx.http.request`、retry/backoff、`ctx.local`、timers）；`docs/contracts/ctx-api.md` 功能集与 Pending 节自称“更长的 v1 计划”。两者叠加等于把“后续切片”写进 1.0 稳定承诺，与 [ADR 0011](0011-contract-compatibility.md) 的“1.0.0 要求配置、CLI、`ctx` API version 1 稳定”冲突。

实现现状（2026-09-30）：`src/script.rs` 只实现 `bodyText`（无 `bodyBytes`）、`ctx.http` 桥接只有 `get`/`pipe`（无 `request`）、`http.get` 只接受 `timeout_ms`（`retries` fail-closed，见 `tests/cli.rs:3842/3879/3889/4315`）、无 `ctx.local`、无 `setTimeout`/`setInterval`。调用不存在成员统一映射为 500 `script_error`。

## 决策

1. **5 项全部移出 1.0。** 1.0 只承诺 T1–T12 已实现子集。
2. **去向：**
   - `ctx.request.bodyBytes`：v1.x 候选（`apiVersion` 1 内增量，见 ADR 0011）。
   - `ctx.http.get` 的 `retries`/`backoff`：v1.x 候选（只对传输层失败生效，上限仍按 ADR 0005）。
   - `ctx.http.request`：v1.x 候选（新增方法，增量兼容）。
   - `ctx.local`：v2 候选（需先与 ADR 0001 的共享状态边界做设计区分）。
   - `setTimeout`/`setInterval`：v2 候选（需先解决 worker 进程事件循环与 ADR 0014 超时强杀的语义）。
3. **兼容规则不变：** 同一 `apiVersion` 内只做增量；移除或改名需新 `apiVersion`。推迟不破坏兼容。

## 后果

**正面**：1.0 承诺可兑现；`pending` 不再是稳定契约的一部分。

**负面（接受）**：首发表达力收窄；用户需用 `bodyText`/`http.get`/`pipe` 组合先行。

## 替代方案

- 5 项随 1.0 发布（拒绝：把未实现写进稳定承诺，1.0 无法闭环）。
- 继续以 `pending` 留在 v1 计划（拒绝：与“功能已收敛”矛盾，定义不闭环）。

# Python 脚本运行时推迟到 v2

- 状态：`已接受`
- 日期：2026-09-30
- 关联：[ADR 0003](0003-script-first-multi-runtime.md)、[ADR 0004](0004-host-functions-only-sandbox.md)、[ADR 0011](0011-contract-compatibility.md)、[ADR 0014](0014-process-isolated-script-runner.md)、[产品功能定义](../product-definition.md)、[脚本运行时选型调研](../../research/script-runtime-selection.md)

## 背景

ADR 0003 把 v1 脚本运行时定为 JS/TS + Python：JS 侧用 Boa，Python 侧用 RustPython（CPython 3.14 兼容子集），并在替代方案里留下“只支持 JS/TS 不支持 Python（待定：可能仍是正确的 v1 选择）”。

v1 收尾时重新评估：Python 侧零实现（`Cargo.toml`/`Cargo.lock`/`src/` 无 RustPython 痕迹，配置校验只接受 `.js`，拒绝 `.py` 并给出真实原因）、无跟踪 issue、无外部需求；`docs/contracts/ctx-api.md` 还有 `bodyBytes`、`ctx.http.request`、retry/backoff、`ctx.local`、timers 未实现，同一套能力要在两个运行时上实现、测试、文档化，收尾成本近似翻倍。

## 决策依据

1. RustPython 官方自述未 production-ready：`README.md` 原话是 “RustPython is not totally production-ready”，嵌入文档仍处 early phase。最新 crate `rustpython-vm 0.5.0`（2026-03-31）仍是 0.x，上一个 0.4.0 在 2024-08-06，相隔约 20 个月；仓库有 400+ open issues。
2. 与本仓库沙箱模型直接相关的上游 RFC（“support embedding and sandboxing untrusted code”，#4210）自 2022 年开放至今；ADR 0004 要求移除 Python 的 `os`/`subprocess`/`socket` 导入能力，这条路在上游没有成熟方案。
3. 接入成本可量化：`rustpython-vm 0.5.0` 要求 rustc 1.93，仓库当前 MSRV 为 1.91，按“MSRV 取依赖树最高要求”政策必须上调；RustPython 的 binary-size RFC（#4203、#5403）至今 open，而当前发布产物约 6.0–6.7 MB；`RLIMIT_AS` 256 MiB 默认值与 64 MiB floor 按 Boa 实测校准（ADR 0014 S3），换运行时要重做内存预算校准。
4. ADR 0011 要求 1.0.0 同时稳定配置、CLI、`ctx` API version 1；Python 一旦进 1.0，其 stdlib 子集、错误语义、平台差异全部成为长期契约。业内同类首发均为单脚本生态（WireMock 的 Java 扩展 + Handlebars 模板、Mountebank 的 JavaScript injection、Mockoon 的模板体系）；多语言诉求的成熟解法是外置 middleware（如 Hoverfly 的 stdin/stdout JSON 或 HTTP middleware），而非在 1.0 二进制里塞第二个解释器。

## 决策

1. **Python 移出 v1，进入 v2 路线。** v1 脚本运行时只有 Boa（JS）；`validate` 继续拒绝 `.py` 并给出真实原因。
2. **1.0 的兼容承诺不含 Python。** `ctx` API version 1 的实现子集、`.d.ts`、配置校验都不承诺任何 Python 语义。
3. **v1.x 不做静默实验。** 如需提前验证，必须以显式实验形态（如 `--experimental-python`）发布，明确不在 1.0 契约内、不承诺 stdlib 对等。
4. **v2 转正条件**（全部满足才进入实现）：真实用户需求出现；RustPython 达到 production-ready（或 1.0）；spike 通过——二进制体积、内存预算重校准、沙箱导入移除、跨平台构建（含 Windows runner）。

## 后果

**正面**：v1 只冻结一套已验证的运行时，1.0 承诺可兑现；收尾工作收敛到 pending `ctx` 能力与发布就绪。

**负面（接受）**：首发失去对 Mockoon/Mountebank 的“内置 Python”差异；ADR 0007 中文辅助定位与产品定义里的 JS/Python 表述需要同步收敛到 JS。

**中性**：worker/ctx 层保持 runtime-agnostic；ADR 0003 的 Python 选型结论保留为 v2 输入，不删除。

## 替代方案

- Python 随 v1 发布（拒绝：把 0.x、未 production-ready 的引擎写进 1.0 稳定契约，风险与 Boa 叠加而非分散）。
- v1.x 静默加入 Python（拒绝：违反 ADR 0011 的契约同步规则；用户会把实验行为当作承诺）。

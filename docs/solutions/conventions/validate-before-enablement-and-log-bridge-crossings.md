---
title: "参数校验先于启用门禁，调用日志只记录跨过宿主桥的调用"
date: 2026-11-08
category: conventions
module: script file write host boundary
problem_type: convention
component: server
severity: medium
applies_when:
  - "新增或复审带启用状态（如 files.writable、host allowlist）的宿主能力，且参数本身可能畸形时"
  - "设计或修改 ctx.file、ctx.http 的脚本到宿主桥，或调整 per-request 调用日志口径时"
  - "同一份非法输入在不同层得到不同错误码，或日志条目数与脚本实际调用次数对不上时"
  - "为 fail-closed 边界补回归测试，需要覆盖“参数非法且能力未启用”的组合时"
related_components: [script, files, upstream, tests]
tags: [host-boundary, validation-order, error-precedence, file-writes, file-calls, observability, context-file, bridge]
---

# 参数校验先于启用门禁，调用日志只记录跨过宿主桥的调用

## Context

`ctx.file.writeText` / `ctx.file.writeBytes` 同时受两层约束：脚本侧的参数形状（字符串、`Uint8Array`/`ArrayBuffer`/稠密字节数组、孤立 UTF-16 surrogate、单次 8 MiB 上限）与宿主的启用状态（`files.writable`，默认关闭）。PR #236 复核 ADR 0022 时发现契约没有把这个先后关系写清楚，留下两种同样“看起来合理”的读法：

- **门禁优先**：写入关闭时任何写入都答 `file_write_disabled`，实现上就是宿主 `write_bytes` 的第一行检查（`src/files.rs:821`）；
- **参数优先**：脚本侧能判定的参数形状错误在任何配置下都答自己的错误码，`file_write_disabled` 只表示“宿主没碰磁盘”。

这两种读法在“写入关闭 + 参数非法”的组合下给出不同错误码，而调用方只能看到其中一个。同一轮复核还发现第二个口径问题：ADR 0022 说“每次写入尝试都进 `file_calls`”，但脚本侧前置校验失败的调用从未跨过宿主桥，日志里根本没有条目；评审一度把这当成漏项。两处都不是实现 bug，而是契约没有说清“哪一层负责判定、哪一层负责记录”。

边界要说准：脚本侧判定的是**形状、编码与尺寸**（`Uint8Array`/`ArrayBuffer`/稠密数组、孤立 surrogate、8 MiB）。**路径合法性**（绝对路径、`..`、symlink 组件）仍由宿主 `resolve_for_write` 在 `write_bytes` 内部判定（`src/files.rs:845`），晚于 `:821` 的启用门禁。因此写入关闭时，非法路径依旧报 `file_write_disabled`——这是已知且可接受的边界，不要据此把结论扩大成“所有参数错误都优先”。

最终采纳的是“参数优先 + 日志按桥记录”，并把结论写进公开契约、ADR 与类型定义，而不是改代码顺序。理由见下。

## Guidance

### 1. 谁看得见输入形状，谁就先校验

脚本侧看得到原始 JS 值：`Uint8Array` 与 `ArrayBuffer` 与稀疏数组的差异、孤立 surrogate、真实的字节长度。宿主只看得到经 `JSON.stringify` 投影后的载荷（写路径传的是 `bytes_base64` 字符串），从那里已经无法区分“容器类型错误”“缺字段”“内容非法”。

因此参数校验必须在脚本侧、`callBridge` 之前完成，宿主的同名检查退化为纵深防御：

```js
// src/script.rs:856-870（fileWriteText）：类型 → 编码 → 尺寸 → 才过桥
checkUtf16Surrogates(text, label + ": text must be well-formed UTF-16", "file_encoding_error");
if (measureUtf8Bytes(text) > 8388608) {
  throw makeError(label + ": text exceeds the 8 MiB per-call write cap", "file_too_large");
}
var result = callBridge(__sd_file, { op: "writeText", ... }, label);
```

`writeBytes` 同理：`src/script.rs:827-854` 依次做容器类型、8 MiB、稀疏数组、`[0, 255]` 整数校验，`callBridge` 在 `src/script.rs:875` 才发生。

这条规则来自实现约束，不是风格偏好：把校验挪到宿主会让“容器类型错误”和“字段缺失”合并成同一个 `script_error`，丢掉脚本可自纠的信息；把校验留在脚本侧后，宿主 `write_bytes` 的 `if !self.writable`（`src/files.rs:821`）只会面对形状已合法的调用，因此启用状态不会遮蔽形状错误。

业界的同构做法：Rust API guidelines 的 C-VALIDATE 要求 API 在入口处拒绝无效输入（“Rust APIs do not generally follow the robustness principle… enforce the validity of input whenever practical”）；Node.js 的 `writeFileSync` 也先做选项与数据校验再进入绑定层，`ERR_INVALID_ARG_TYPE` 先于 `EACCES`。

### 2. 错误码应指向失败的那一层

`file_write_disabled` 是**配置**错误，`file_path_invalid` / `file_too_large` / `file_encoding_error` / `script_error` 是**调用**错误。若门禁优先，一个拼错的参数在默认配置下会被报成“写入未开启”，运维会去改 TOML，而真正的缺陷留在脚本里。

契约因此写成“通过参数校验的写入调用在触碰磁盘前抛 `file_write_disabled`”，并显式说明参数校验与 8 MiB 检查先于启用状态检查（`docs/contracts/ctx-api.md:76-77`、`types/ctx-api-v1.d.ts:138-142`、`plans/adr/0022-script-file-writes.md:39`）。

回归锁在端到端层：`tests/cli.rs:4648-4674` 在写入关闭的服务器上分别发送孤立 surrogate 与 `[256]`，断言得到 `file_encoding_error` 与 `TypeError`，同时断言两个文件都没被创建（`tests/cli.rs:4669-4674`）。只测“关闭时答 `file_write_disabled`”会漏掉整个优先级问题。

### 3. 调用日志的边界是桥，不是磁盘

`file_calls` 的记录槽位在宿主 `FileAccess::call` 进入后创建（`src/files.rs:725`），早于载荷字段校验（`src/files.rs:743-757`）。因此：

- 跨过桥但载荷非法的调用**会**入账，错误码为 `script_error`；
- 在脚本侧被前置校验拦下、从未调用 `callBridge` 的调用**不会**入账。

这与 `ctx.http.get` 的既有口径一致：`UpstreamAccess::get` 先 `begin_call`（`src/upstream.rs:282-283`），再 `parse_get_call`（`src/upstream.rs:296-297`，校验实现自 `src/upstream.rs:545`）；而 `httpGet` 的 URL / `timeout_ms` 校验在 `src/script.rs:788-800` 的桥之前完成。

这不是“漏记”，而是把日志定义成**宿主边界穿越账本**。它带来两个可断言的性质：条目数等于真正到达宿主的调用数；每条记录的错误码都来自宿主侧，脚本错误码不会伪装成宿主错误码。ADR 0022 与 CLI 契约已按此表述（`plans/adr/0022-script-file-writes.md:98`、`docs/contracts/cli.md:60`）。

可观测性上也有依据：OWASP Logging Cheat Sheet 的 “Data to exclude” 明确排除 access token、认证密码与 PII；请求/响应体被列在 “Extended details” 的谨慎记录项而不是排除清单里。无论按哪一条，穿越账本都只需保留稳定分类与元数据：它只含 `api`、`bytes`、`duration_ms` 与稳定 `error`，路径、内容、客户端文件名都不入账。

### 4. 已确认的配套边界（本文只索引，不重复其论证）

- **deadline**：写入与替换重试都受脚本 deadline 约束，超时后不创建也不替换（`src/files.rs:841`、`:846`、`:1048`、`:1063-1066`）；单次调用与重试窗口都要检查，否则超时请求留下不可观察的副作用。
- **并发测试空转**：barrier 只保证开始时刻对齐，写者可能在读者进入循环前跑完；因此要求每个读者完成最小循环次数（`src/files.rs:1434`、`:1474`、`:1493`、`:1507-1509`）。
- **Windows 重试事实**：`tempfile` 的 `persist` 直接 `MoveFileExW`，库层不重试；宿主最多 3 次尝试（`src/files.rs:1085`）只是平台补偿，last-writer-wins 不变。

## Why This Matters

错误优先级决定脚本作者能否自纠。把参数错误报成配置错误，会让“服务端没开写入”和“我的字节数组写错了”在客户端完全不可区分，调试方向被系统性带偏；而这类混淆一旦进入契约，后续再想纠正就要改变用户已经依赖的错误码。

日志口径决定日志能否被信任。若把桥前拒绝也补进 `file_calls`，日志就从“宿主边界穿越账本”退化成“脚本调用流水”，宿主侧的 `bytes`、`duration_ms`、稳定错误码都无从填充，只能填伪造值；而若干脆只记成功调用，失败的可观测性又消失。把边界钉在桥上是唯一能同时保住“可分类”和“可核对”的位置。

`ctx.http.get` 已经先走了这条路，两处口径不一致会让 `file_calls` 与 `upstream_calls` 在同一个请求日志里表达不同含义，跨 API 的对账失效。

## When to Apply

- 新增宿主能力且带启用开关（写入、外呼 allowlist、上传、状态提交）时，先决定校验层与门禁层的顺序，再写实现。
- 复审 `src/script.rs` 的 prelude 与 `src/files.rs` / `src/upstream.rs` 的宿主入口时：确认“校验在桥前、记录在桥后”没有被重构打乱。
- 调整 per-request 日志字段时：任何新增的 `*_calls` 数组都应回答“记录的是调用意图，还是跨过边界的事实”。
- 写 fail-closed 回归测试时：参数非法 × 能力未启用是两个维度，不能只覆盖其中一个。
- 排查“脚本调用成功但日志缺条目”或“日志有 `script_error` 但脚本捕获的是别的码”时，先看记录槽位在哪一层创建。

## Examples

### 反例：门禁先于校验（本轮复核提出后又被否决的读法）

PR #236 的复核一开始把 ADR 0022 读成“门禁优先”，据此认为实现漏了校验顺序与日志条目。最终结论相反：契约被改成参数优先，代码顺序保持不变。若真按门禁优先，默认配置下会得到：

```text
ctx.file.writeBytes("a.bin", [256])   → file_write_disabled   （应为 TypeError）
ctx.file.writeText("a.txt", "\uD800") → file_write_disabled   （应为 file_encoding_error）
```

两个调用都被误报成配置问题，且日志里没有任何条目——因为确实没有跨桥。

### 正例：校验、门禁、记录三层各就各位

```text
脚本 prelude 校验失败 ──► 抛 TypeError / file_encoding_error / file_too_large
                         （不入 file_calls，从未跨桥）
        │ 校验通过
        ▼
callBridge ──► FileAccess::call ──► begin_call（入账）
                              │ 载荷仍非法 ──► script_error（已入账）
                              │ 合法但未启用 ──► file_write_disabled（已入账，未碰磁盘）
                              │ 已启用 ──► 落盘或 file_io_error（已入账）
```

对应的断言：`tests/cli.rs:4669-4674` 锁住第一行的两个错误码与“文件未创建”；`src/files.rs:1302-1317` 锁住第三行的 `file_write_disabled` 与日志条目。

相关文档：[脚本文件写入是 artifact output 而非 Shared state](script-file-writes-are-artifacts-not-shared-state.md) 覆盖写入语义、路径分叉与原子替换；[宿主边界输入分类](host-boundary-fail-closed-input-classification.md) 覆盖读取侧的缺失/畸形/不可用三态；[脚本侧预检与宿主权威校验](host-authoritative-state-bridge-validation.md) 覆盖状态提交的同类分工；[上游阻塞 body 到异步流的桥](../architecture-patterns/blocking-upstream-body-to-async-stream-bridge.md) 覆盖流式响应的定稿时机（`begin_call` 先于参数校验的实现见 `src/upstream.rs:282-283`）。

---
title: Calibrate Linux RLIMIT_AS from the final worker peak and classify OOM by allocator signature
date: 2026-09-28
category: architecture-patterns
module: script worker process isolation
problem_type: architecture_pattern
component: server
severity: medium
applies_when:
  - "Setting or reviewing `sandbox.script_memory_limit_mb` for Linux script workers."
  - "Changing the worker startup shape, IPC or serialization buffers, or the supported buffered file-read cap."
  - "Classifying worker death or engine errors as memory-limit exhaustion versus a normal script error."
  - "Deciding whether RLIMIT_AS headroom is sufficient for the largest documented buffered file read."
  - "Reviewing platform differences where Linux enforces RLIMIT_AS but macOS and Windows do not provide this hard bound."
related_components:
  - script
  - config
  - tests
retire_when: "Boa changes its allocator-failure messages; recheck the Boa release notes and rerun the Linux OOM-attribution tests before keeping the exact-match rule."
tags:
  - rlimit-as
  - process-isolation
  - script-worker
  - memory-limit
  - virtual-address-space
  - oom-attribution
  - allocator-error
  - ctx-file
---

# 用最终 runner 峰值校准 Linux script worker 的 `RLIMIT_AS` 预算

## 背景（Context）

[issue #28](https://github.com/geeknonerd/stuntdouble/issues/28) 需要给 Route 脚本建立硬内存与 deadline 边界；[ADR 0014](../../../plans/adr/0014-process-isolated-script-runner.md) 因此采用每请求一个新 script worker，并在 Linux worker 启动早期用 `RLIMIT_AS`（soft = hard）限制整个进程的虚拟地址空间（`plans/adr/0014-process-isolated-script-runner.md`；`src/script.rs` 的 `worker_main` / `apply_worker_memory_limit`）。

设计阶段曾在一个最小单线程 Boa runner 上测得约 15.2 MiB VMS，并在 `RLIMIT_AS=64 MiB` 下跑通简单脚本；该结果被当作 64 MiB 默认值可行的证据（session history）。但这条基线不包含完整 `serve` 的 HTTP/Tokio、IPC、`serde_json` 和协议缓冲，不能外推到最终 worker 形态。最终 runner 的初步实测随后推翻了这个预算：Linux 本机 debug/release 下，普通脚本最低约 40/24 MiB，而受支持的 8 MiB `ctx.file.readText` 的最低可用 `RLIMIT_AS` 约 192 MiB（`plans/adr/0014-process-isolated-script-runner.md`；`research/process-isolation-assessment.md`）。正式 S3 复测又测得该路径的 `VmPeak` 为 173.1 MiB，五条 demo 路由为 21.0–21.1 MiB，默认限制下的 memory bomb 为 255.9 MiB（[#93 评论](https://github.com/geeknonerd/stuntdouble/issues/93#issuecomment-5880243728)）。放大来自最终链路叠加的 IPC、`serde_json`、Boa 字符串以及协议缓冲，而不是 Boa 引擎单独需要这么多内存。

预算和归因是一体的两半：如果 worker 因内存边界死亡，却把所有 Boa `RangeError` 都报成“配置内存限制”，就无法判断边界是正确生效还是误杀了普通脚本错误。前序会话没有建立 OOM 与 `RangeError` 的区分判据（session history）；[issue #91](https://github.com/geeknonerd/stuntdouble/issues/91) 的实现评审因此补齐了精确归因，并用正反单元测试锁定。

| 观测口径 | 实测/当前值 | 解释 |
| --- | --- | --- |
| 仅 Boa + 简单脚本 | 约 15.2 MiB VMS | 设计阶段的旧基线，不含 IPC、序列化和协议缓冲，不能直接推导最终 runner 的安全默认值（`research/process-isolation-assessment.md` 的预算复测与 S3 条目）。 |
| 最终 runner，五条 demo 路由 | release `VmPeak` 21.0–21.1 MiB | 覆盖本地/upstream manifest、本地/upstream download 与 upload；floor 至少要覆盖普通 worker 启动与简单脚本。 |
| 最终 runner，8 MiB `ctx.file.readText` | `VmPeak` 173.1 MiB；最低可用 `RLIMIT_AS` 约 192 MiB | buffered read 的字节会经过多份临时表示；文档化的 8 MiB 上限并不等于 worker 内存预算只有 8 MiB（`src/files.rs` 的 8 MiB read cap；`SECURITY.md`）。 |
| 当前默认值 / floor | 256 MiB / 64 MiB | 默认值比 192 MiB 最低可用校准高 33%，相对 173.1 MiB 观测峰值留出 82.9 MiB（32.4%）余量；floor 覆盖普通脚本，而不是所有受支持操作的峰值（`src/config.rs`；`docs/contracts/config.md`）。 |

修复已由 [#98](https://github.com/geeknonerd/stuntdouble/pull/98) 合并并随 v0.5.0 发布。正式 S3 测量、默认值复核与 #28 验收报告由 [issue #93](https://github.com/geeknonerd/stuntdouble/issues/93) 于 2026-09-29 完成；因此本文的数字是最终验收依据（原始汇总见 [#93 评论](https://github.com/geeknonerd/stuntdouble/issues/93#issuecomment-5880243728)）。

## 指导（Guidance）

1. **按最终 runner 的峰值预算，而不是按引擎基线外推。** 预算要包含 worker 启动、请求快照、IPC、序列化/反序列化、Boa 字符串和协议缓冲。`RLIMIT_AS` 约束的是虚拟地址空间，不是 RSS；这些临时副本全部计入（`docs/contracts/config.md` 的虚拟地址空间语义）。旧的 15.2 MiB 只测 Boa 与简单脚本，不能作为最终默认值的安全下界（`research/process-isolation-assessment.md` 的预算复测与 S3 条目）。

2. **把 floor 与 default 分成两个问题。** floor 回答“普通脚本能否启动并运行”；default 回答“文档支持的 buffered read 能否完成”。当前 64 MiB floor 有专门的普通脚本回归（`tests/cli.rs` 的 `ordinary_script_runs_at_configured_memory_floor`），但 8 MiB `ctx.file.readText` 不能再在 64 MiB 下测试或宣称可用；其回归应在默认 256 MiB 配置下运行（`tests/cli.rs` 的 `ctx_file_reads_cap_at_eight_mib`）。降低默认值到 floor 会重新落回 192 MiB 最低可用校准以下。

3. **在读取 job 和解析请求快照之前应用上限。** 当前实现先校验 floor，再调用 `apply_worker_memory_limit`，之后才读取协议第一行并执行 `serde_json` 解析（`src/script.rs` 的 `worker_main` / `read_protocol_line`）。这样预算覆盖“解析大脚本或请求快照”本身的分配；若只在脚本 eval 前设置，启动与 IPC 峰值会漏算。

4. **OOM 归因必须窄，不能把所有 Boa `RangeError` 都当成配置内存上限。** 只有 Boa 0.22 的 allocator-owned layout failure，以及 Rust allocator 的精确 abort 行，才映射为 `MemoryLimitExceeded`：前者要求 native `RangeError` 同时满足 `invalid layout ` 前缀和 ` while allocating data block` 后缀（`src/script.rs` 的 `engine_out_of_memory`）；后者要求整行匹配 `memory allocation of ... failed`（`src/script.rs` 的 `is_allocator_abort_line` / `unexpected_worker_error`）。`capacity overflow`、`couldn't allocate the data block: out of range`、其他普通 `RangeError` 仍必须是普通脚本错误（`src/script.rs` 的 `engine_oom_errors_are_classified_as_memory_limit_errors`）。

5. **改动默认值时同步重算并发总量，而不是把它当 RSS 预留。** ADR 0014 的总量公式是“单进程上限 × 槽位数”（ADR 0014 D12）；契约中的槽位池是 4–16（`docs/contracts/ctx-api.md` 的 worker 槽位）。256 MiB 对应的是 1–4 GiB 的虚拟地址空间上限量级，而不是会立即占用的物理内存。

## 为什么重要（Why This Matters）

- **旧默认值会误杀文档支持的正常操作。** 64 MiB 甚至低于 8 MiB `ctx.file.readText` 约 192 MiB 的最低可用 `RLIMIT_AS`（观测 `VmPeak` 为 173.1 MiB）；这不是脚本恶意分配，而是 IPC 与多层字符串/JSON 表示的正常放大。把 floor 当 default 会让受支持用例在 Linux 上稳定失败。
- **引擎基线会制造错误的安全感。** 15.2 MiB VMS 只覆盖 Boa 与简单脚本；最终 runner 的协议和宿主调用路径使峰值与引擎基线脱钩。继续用旧数字外推，会重复 #91 的预算错误。
- **错误的归因会掩盖真正的脚本错误。** `capacity overflow` 等 `RangeError` 可能来自普通脚本逻辑或数据形状错误；把它们标成“配置内存限制”会误导脚本作者和运维，并把客户端可见的稳定 detail 变成错误结论（`src/script.rs` 的 `Error::MemoryLimitExceeded`）。
- **预算和平台边界必须一起说明。** Linux 才执行 `RLIMIT_AS`；macOS 与 Windows 保留进程隔离和 deadline kill，但没有硬内存上限（`docs/contracts/config.md`、`SECURITY.md`）。不能把 Linux 的 256 MiB 默认值描述成跨平台的内存保证。

## 何时适用（When to Apply）

- 调整 `sandbox.script_memory_limit_mb` 的默认值、floor、校验范围或 `RLIMIT_AS` 设置时机时。
- 修改 script worker 的 IPC/JSON Lines 协议、请求快照序列化、Boa 字符串桥接、`ctx.file.readText` / `readBytes` 的 8 MiB 上限，或任何会改变 worker 峰值分配的功能时。
- 升级 Boa，尤其是需要重新验证 allocator failure 文本、`RangeError` 形状或 `AlignedVec` 归因时；当前匹配规则带有 Boa 0.22 版本前提（`src/script.rs` 的 `engine_out_of_memory`）。
- 评估并发槽位池或整体资源上限时；默认值变化会改变 ADR 0014 的“单进程上限 × 槽位数”总量口径（ADR 0014 D12）。
- 只适用于需要 Linux 硬内存边界的 worker。macOS/Windows 没有硬上界，不能用本文的 256 MiB/64 MiB 结论替代平台专属的 deadline 与隔离验证。

## 示例（Examples）

### 1. 配置与 floor 的回归锚点

当前单元测试把默认值锁在 256 MiB，并接受 64/128/256、拒绝 0、负数、15、16、63、浮点和字符串（`src/config.rs` 的 `sandbox_memory_limit_*` 单测）。CLI seam 同样在 `validate` 层覆盖 64 的接受和 floor 以下的拒绝，错误信息必须指出字段与 `64`（`tests/cli.rs` 的 `validate_accepts_sandbox_memory_limit` / `validate_rejects_sandbox_memory_limit_below_the_floor`）。这些断言保证“默认值调整”不会只改实现而漏掉公开契约。

### 2. 两个不同层次的 worker 测试

```rust
// 64 MiB floor：普通脚本应能继续返回 200。
let config_body = with_sandbox_bounds(good_config(), 10_000, 64);
let (response, _) = serve_and_run(/* ... */);
assert_eq!(response.status, 200, "body: {}", response.body);
```

该结构对应当前 `ordinary_script_runs_at_configured_memory_floor`（`tests/cli.rs` 的 `ordinary_script_runs_at_configured_memory_floor`）。它证明 floor 对普通启动路径成立，不证明 64 MiB 足以完成 8 MiB buffered read。

```rust
// 64 MiB 限额下的内存炸弹：必须被停止、映射为 500 script_error，
// 且 --verbose 只输出稳定的内存 detail。
assert_eq!(response.status, 500, "body: {}", response.body);
assert_eq!(
    json_string(&response.body, "detail").as_deref(),
    Some("script exceeded the configured memory limit"),
    "body: {}",
    response.body
);
```

完整回归还检查停止时间有界、worker 子进程消失、响应不泄露脚本文本/`ArrayBuffer`（`tests/cli.rs` 的 `memory_bomb_is_stopped_and_reported_by_the_configured_limit`）；不加 `--verbose` 时 detail 必须完全省略（`tests/cli.rs` 的 `memory_limit_detail_is_hidden_without_verbose`）。稳定文本由 `Error::MemoryLimitExceeded` 提供（`src/script.rs` 的 `Error::MemoryLimitExceeded`）。

### 3. 归因守门代码与反例

```rust
fn engine_out_of_memory(error: &JsError) -> bool {
    error.as_native().is_some_and(|native| {
        matches!(native.kind(), JsNativeErrorKind::Range)
            && native.message().starts_with("invalid layout ")
            && native.message().ends_with(" while allocating data block")
    })
}

fn is_allocator_abort_line(line: &str) -> bool {
    line.starts_with("memory allocation of ") && line.ends_with(" failed")
}
```

这正是当前实现的匹配规则（`src/script.rs` 的 `engine_out_of_memory` / `is_allocator_abort_line`）。单元测试同时断言以下输入为 `false`，防止把普通脚本错误升级成内存归因：`capacity overflow for size ...`、`couldn't allocate the data block: out of range` 和无关的 `RangeError`（`src/script.rs` 的 `engine_oom_errors_are_classified_as_memory_limit_errors`）；worker 已死亡时，只有 Linux 且 stderr 出现精确 allocator abort 行才映射为内存上限（`src/script.rs` 的 `unexpected_worker_error`）。

### 4. 受支持 buffered read 的默认预算回归

`ctx_file_reads_cap_at_eight_mib` 在默认配置下写入恰好 8 MiB 和 8 MiB + 1 的文件，断言前者成功读取、后者以 `file_too_large` 被拒绝（`tests/cli.rs` 的 `ctx_file_reads_cap_at_eight_mib`）。它把 8 MiB 公共契约、默认 256 MiB 预算和 worker 的实际运行路径串在一起；任何再次把默认值降到 192 MiB 以下、或把 read cap 放大却不重测内存的改动，都可能在这条链路上失败；S3 复核已确认当前 256 MiB 默认值可行，后续改动仍需重新实测。

## 相关（Related）

- [macOS 无法用 `RLIMIT_AS` 建立有用的硬内存上界](./macos-rlimit-as-cannot-enforce-useful-memory-bound.md)：同一进程隔离边界的平台对照；macOS 保留进程隔离与 deadline，但不承诺硬内存上限。
- [在契约上限处测试 script worker 容量，并证明 deadline 后槽位可复用](../conventions/script-worker-contract-max-and-post-deadline-held-slot-tests.md)：同一 worker 的容量与生命周期边界；内存预算与容量测试是两套互补验证。
- [把阻塞上游 body 桥接为异步流响应](./blocking-upstream-body-to-async-stream-bridge.md)：大 body 走 streaming 而不是进入脚本堆；本文记录的是受支持 buffered read 的互补内存成本。
- [ADR 0014：进程隔离脚本执行](../../../plans/adr/0014-process-isolated-script-runner.md) 与 [进程隔离方案评估](../../../research/process-isolation-assessment.md)：D6 的平台口径、S3 正式证据与历史 64 MiB 假设的权威出处。
- [配置契约](../../contracts/config.md)、[`ctx` API 契约](../../contracts/ctx-api.md) 与 [SECURITY.md](../../../SECURITY.md)：默认值、floor、8 MiB buffered read 上限、平台差异和稳定错误 detail 的公开承诺。

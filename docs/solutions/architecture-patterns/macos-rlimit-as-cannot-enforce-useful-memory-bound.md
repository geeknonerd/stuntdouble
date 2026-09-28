---
title: "macOS cannot enforce a useful RLIMIT_AS hard memory bound"
date: 2026-09-28
category: architecture-patterns
module: script worker process isolation
problem_type: tooling_decision
component: server
severity: medium
applies_when:
  - "Designing or documenting Unix script-worker memory limits under process isolation"
  - "Relying on RLIMIT_AS to bound a macOS or Apple Silicon child process"
  - "Deciding whether to raise the configured memory limit when setrlimit(RLIMIT_AS) fails"
  - "Explaining why macOS gets process isolation and deadline kill but no hard memory bound"
related_components: [script, ci]
retire_when: "macOS/XNU changes setrlimit(RLIMIT_AS) semantics so a limit below the child's pre-existing VM map can enforce a useful bound; recheck Darwin/XNU release notes or rerun the standalone probe on a current macOS runner"
tags: [macos, rlimit-as, setrlimit, process-isolation, script-worker, memory-limit, virtual-address-space, apple-silicon]
---

# macOS 无法用 `RLIMIT_AS` 建立有用的硬内存上界

## 背景

[issue #28](https://github.com/geeknonerd/stuntdouble/issues/28) 要求为 Route 脚本建立硬内存与 deadline 边界。[ADR 0014](../../../plans/adr/0014-process-isolated-script-runner.md) 采用每请求新 **script worker** 的进程隔离设计，并用 deadline 强杀承担 CPU 与超时边界（`plans/adr/0014-process-isolated-script-runner.md:18`、`:24`）。D6 原先把 Unix（Linux/macOS）放在同一套 `RLIMIT_AS` 策略下：默认 64 MiB，配置键 `sandbox.script_memory_limit_mb`（`plans/adr/0014-process-isolated-script-runner.md:25`）。领域含义见 `CONTEXT.md:31-36`（Script sandbox / Script worker）。

设计阶段掌握的边界只到「限制值低于当前虚拟内存映射时 macOS 会 `EINVAL`」，并据此假设 64 MiB 可行（见 [方案评估](../../../research/process-isolation-assessment.md) §5.3 的前置验证清单）。#89 把这条假设变成可复现探针：在临时 release example 中复用生产 Boa 评估路径，子进程启动早期调用 `setrlimit`，父进程从进程外采样。证据见 [issue #89](https://github.com/geeknonerd/stuntdouble/issues/89)、[原始测量记录](https://github.com/geeknonerd/stuntdouble/issues/89#issuecomment-5856621521) 与 [CI 运行 36324984631](https://github.com/geeknonerd/stuntdouble/actions/runs/36324984631)。

探针返工留下了这条复现纪律：早期 S1 门禁没有单独断言 `setrlimit` 是否真正生效；只有「脚本跑完了」不能排除「限制设置失败」。修正后的探针把「限制是否真正生效」与「采样是否有效」拆成显式门禁：S1 预期限制失败时仍要求采样有效，S2 请求了限制时断言设置成功；#89 评论记录了最终通过的门禁与结果。

当前工作树（`feat/90-process-script-worker`，尚未合并到 `main`）已落地进程隔离：脚本在每请求独立的 worker 进程里执行，deadline 到期由父进程强杀并回收（`src/script.rs:1254-1348`、`docs/contracts/ctx-api.md:79`）；但**内存硬限仍未实现**：`sandbox` 解析器只接受 `script_timeout_ms`（`src/config.rs:292`），公开配置契约也只列该键（`docs/contracts/config.md:31`），代码里没有 `setrlimit` 或 `script_memory_limit_mb`。实现票 [#91](https://github.com/geeknonerd/stuntdouble/issues/91)（Linux 硬限）、[#93](https://github.com/geeknonerd/stuntdouble/issues/93)（预算复测与契约同步）、[#92](https://github.com/geeknonerd/stuntdouble/issues/92)（孤儿回收）仍待完成，所以下文是待落地设计的平台边界，不是已发布行为。

## 指导

1. **先做逐平台探针，再写「硬限制」承诺。** 对任何跨平台 sandbox 资源边界，在 ADR、config、SECURITY 或 README 承诺之前，先在每个目标 OS/架构上实际调用该 primitive；成功标准是系统调用成功且边界确实生效，不是子进程还能跑完。
2. **macOS 上不能用 `RLIMIT_AS` 获得有用的硬内存上界。** 限制必须高于子进程启动后已有的 VM map；几十 MiB 到 64 GiB 都低于该 map，被 XNU 以 `EINVAL`（errno 22）拒绝。不要靠调高默认值绕过：64 GiB 仍失败；在已测四档中仅 512 GiB 可设置，但已不构成有效边界。macOS 的脚本内存故事是「进程隔离 + deadline 强杀」，并显式记录「无硬内存上界」的平台差异（ADR 0014 D6/D7，`plans/adr/0014-process-isolated-script-runner.md:25`、`plans/adr/0014-process-isolated-script-runner.md:54`）。Linux 继续使用 `RLIMIT_AS`。
3. **若未来 macOS 必须要有内存上界，再评估引擎级限制。** ADR 保留的 rquickjs `set_memory_limit` 只能约束解释器分配，不覆盖 Rust 侧缓冲，是降级方案而不是等价物（`plans/adr/0014-process-isolated-script-runner.md:59`）。实现票 #91 应只承诺 Linux 硬限，最终契约同步由 #93 处理；在它们完成前不要宣称 `sandbox.script_memory_limit_mb` 已可用。票面已对齐：spec #88 与实现票 #91 都写明硬限仅 Linux、macOS 保持「进程隔离 + deadline 强杀」，不再有 macOS 假保证。（实现后记：#91 按仓库规则已同步公开契约；最终预算证据与验收归档由 #93 完成。）
4. **复现时从进程外测量，并记录失败原因。** 子进程先初始化生产同形的脚本运行时，再尽早调用 `setrlimit(RLIMIT_AS)`；父进程每隔约 2 ms 用 `ps -o vsz=,rss= -p <pid>` 采样，同时记录 `setrlimit` 返回值、errno 与峰值 VSZ/RSS，并按 64 MiB → 512 MiB → 64 GiB → 512 GiB 逐档放大，分别判断「能否设置」与「是否形成有用边界」。

## 为什么重要

这是一次「配置或架构写了边界、平台却执行不了」的假保证风险。ADR 0014 原先假设 Linux/macOS 都能用 `RLIMIT_AS` 承担硬内存边界，S1 否定了 macOS 那半边。照原假设发布，macOS 实现要么在设置失败时无法兑现承诺，要么静默忽略 `EINVAL`——两种结果都会让 memory bomb 落在错误的安全假设上。

进程隔离仍提供崩溃隔离，deadline 强杀仍提供 CPU/超时边界，但都不能替代单进程内存上限；并发槽位是总量缓解，不是每个 worker 的硬上限。平台差异必须进入设计、issue 与公开契约，而不是只留在一次性的 CI 日志里。ADR 0014 D6/D7 与 [方案评估 §5.3](../../../research/process-isolation-assessment.md) 修订已把这条收敛结论提交进仓库（PR #94）；最终 runner 形态的预算复测（S3）仍由 #93 待办（`plans/adr/0014-process-isolated-script-runner.md:78`）。

## 何时适用

- 设计或评审进程隔离 sandbox 的 OS 级内存、CPU 或句柄上限，准备把它写成跨平台保证时。
- 目标包含 macOS（尤其 Apple Silicon / `macos-15`），而实现对子进程调用 `setrlimit(RLIMIT_AS)` 时。
- 探针收到 `EINVAL`，正在判断「报错能否忽略」「要不要调高默认上限」「要不要换引擎取得 macOS 内存上界」时。
- 更新 `sandbox.script_memory_limit_mb` 配置、安全文档或平台差异说明时（实现与契约同步属 #91、#93）。

## 示例

实测边界（GitHub `macos-15` arm64，Apple M1 Virtual，macOS 15.7.9；#89）：

| 限制 | `setrlimit(RLIMIT_AS)` | 结论 |
| --- | --- | --- |
| 64 MiB | 失败，`EINVAL`（22） | 低于启动 VM map，不能形成边界 |
| 512 MiB | 失败，`EINVAL`（22） | 调高仍不足以越过 VM map |
| 64 GiB | 失败，`EINVAL`（22） | 排除「只是默认值太小」 |
| 512 GiB（524288 MiB） | 成功 | 可设置，但远高于约 391.6 GiB 的 VM map，不构成有用上界 |

`setrlimit` 前自测的 VM map 为 `410,607,376–410,616,592 KiB`（约 391.6 GiB）；父进程采样到的峰值 RSS 只有 11–12 MiB，说明拒绝来自虚拟地址空间口径，不是物理内存压力。原始数据与解释见 [issue #89 评论](https://github.com/geeknonerd/stuntdouble/issues/89#issuecomment-5856621521)。

一次性探针的命令形状（harness 只存在于不合并 `main` 的 `ci/89-process-isolation-probes` 分支）：

```bash
cargo build --release --example spike89 --locked --features spike-89
python3 scripts/spike89/macos-rlimit-probe.py \
  --binary ./target/release/examples/spike89 \
  --runs 3 --hold-ms 2000 --sample-ms 2 --rlimit-mib 64
```

复跑更高档位时重复 `--rlimit-mib`；要证明的是子进程内 `setrlimit` 确实返回成功且限制生效，而不是只看到脚本完成。

## 相关

- [在契约上限处测试脚本 worker 容量，并证明 deadline 后槽位可复用](../conventions/script-worker-contract-max-and-post-deadline-held-slot-tests.md)：同一 script worker 资源边界主题；该文已刷新技术细节，并明确当前分支尚未建立 `RLIMIT_AS`/RSS 硬限——平台差异以本文与 ADR 0014 D6/D7 为准。
- [Serve 的第二次关闭信号只在第一次被观测后才有保证](../conventions/serve-shutdown-second-signal-semantics.md)：同一原则的另一例——公开承诺要停在操作系统真正保证的边界内。
- [ADR 0014：进程隔离脚本执行](../../../plans/adr/0014-process-isolated-script-runner.md) 与 [进程隔离方案评估](../../../research/process-isolation-assessment.md)：决策与证据的权威出处。

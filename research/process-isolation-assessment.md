# 进程隔离方案评估

- 日期：2026-09-27
- 关联：issue #28（Process-isolated script runner for hard memory and CPU bounds）
- 外部事实依据：[process-isolation-external-facts.md](process-isolation-external-facts.md)（crate 生态、平台语义、cargo-dist、孤儿治理）
- 状态：已完成（含轻量替代方案的全网检索结论）

## 1. 背景与目标

Boa 0.22 无 interrupt hook、无堆内存观测点（ADR 0003 T3 修订），当前进程内执行的已知缺口：

- 超时后 worker 线程无法取消，靠循环上限（1 亿次）兜底回收，期间继续占槽、占内存。
- 无内存硬上限；`#29` 修订用并发上限缓解总量，但不是硬边界。

issue #28 的目标：用 OS 级机制给出硬内存（~64MB）与硬超时边界，保持 `match → source → transform → response` 模型、`ctx` API 契约与脚本源码兼容，并覆盖崩溃隔离、孤儿治理与 pipe 流语义。

## 2. 已定决策（两轮设计访谈，用户按推荐确认）

| # | 决策 | 内容 |
| --- | --- | --- |
| D1 | 隔离边界 | 宿主能力（http/file/upload）全部留在主进程；子进程只跑 Boa，经 RPC 请求能力 |
| D2 | 进程粒度 | 每请求一个新子进程，用完即退；不做进程池复用 |
| D3 | 二进制形态 | self-spawn：同一二进制的隐藏子命令，不作为公开 CLI 契约 |
| D4 | 路径策略 | 单轨替换：删除进程内 `spawn_blocking` 执行路径，所有平台统一进程隔离 |
| D5 | 超时处置 | 到期立即强杀（SIGKILL / TerminateProcess），无宽限期 |
| D6 | 内存口径 | 默认 64 MiB、配置键 `sandbox.script_memory_limit_mb`、子进程启动早期自设 `RLIMIT_AS`（soft=hard）；口径为虚拟地址空间（非 RSS）；配置值设下限保护（macOS EINVAL 边界） |
| D7 | Windows | 内存硬限与不安全 FFI 先延后；Windows 先获得进程隔离 + 超时强杀，内存限制文档化平台差异（用户约束：优先避免 unsafe、Linux/macOS 优先） |
| D8 | IPC 形状 | stdin/stdout + JSON Lines，三类消息（host_call / host_result / final_result），严格一配一；stderr 留子进程诊断 |
| D9 | 孤儿治理 | Unix：子进程监视线程轮询 `std::os::unix::process::parent_id()`（1s 粒度，安全 API）；优雅关闭时主进程显式 kill 在途子进程；不用 `PR_SET_PDEATHSIG` |
| D10 | 错误细分 | `--verbose` detail：超时保持原文；新增 `"script exceeded the configured memory limit"`、`"script worker terminated unexpectedly"`；客户端仍 500 `script_error` |
| D11 | 关闭协调 | 首次关闭信号后等待在途请求在各自 timeout 内自然完成；排空收尾时兜底 kill 存活子进程；不提前杀 |
| D12 | 并发上限 | 保留 `clamp(4, 16)` 槽位公式，语义改为"同时运行的脚本进程数上限"；总内存天花板 = 64 MiB × 槽位数 |
| D13 | CPU 硬限 | 由 D5 父进程 deadline 强杀承担；暂不加 `RLIMIT_CPU`（避免双计时器语义打架，ADR 记录理由） |

## 3. 代码侧改造评估

### 3.1 改造接触面（基于当前代码事实）

| 文件 | 现状 | 改造内容 | 量级 |
| --- | --- | --- | --- |
| `src/script.rs`（1265 行） | 5 个 Boa native bridge → thread-local 直调 upstream/files | bridge 的 `host_call` 实现从"闭包直调"改为"IPC 往返"；`evaluate` 其余部分（prelude、Context、runtime limits、EXTRACT、parse_host_record、错误分类）全部保留并在子进程复用 | 中 |
| `src/server.rs`（1044 行） | Semaphore 槽位 + `script::execute` | 调用点基本不动；execute 内部改为 spawn/监督子进程 | 小 |
| `src/upstream.rs`（871 行） | `UpstreamAccess::get/pipe` | 逻辑零改动，调用方从宿主线程变为 supervisor dispatch | 零 |
| `src/files.rs`（1080 行） | `FileAccess`/`UploadAccess` | 同上 | 零 |
| `src/main.rs`（约 200 行） | serve/validate | 新增隐藏子命令与 runner 入口（不加载配置） | 小 |
| `src/config.rs`（810 行） | sandbox 段 | +`script_memory_limit_mb`（默认 64，下限校验） | 小 |
| 新增模块（IPC + supervisor + runner） | — | 消息类型、主进程监督循环、子进程 runner、孤儿轮询 | 400–600 行 |
| `Cargo.toml` | — | +`rlimit`（MIT，Unix-only）；tokio full 已含 process；零 unsafe 新增 | 极小 |
| 测试 `tests/cli.rs`（5277 行） | 现有契约测试 | 现有测试预期不需改；新增 OOM、超时无残留、崩溃映射、pipe 超时 | +150–250 行 |
| 文档 | config/ctx-api/CHANGELOG/SECURITY/README | 新配置键、平台差异、错误 detail、威胁模型同步 | 小 |

### 3.2 为什么复杂度低于直觉

1. **宿主桥形状已同构**：现在的每次 host 调用已经是 JS `JSON.stringify(payload)` → Rust 解析 → Rust 序列化 → JS `JSON.parse`。IPC 直接复用同一 payload/result JSON 形状，只把"闭包调用"换成"写一行、读一行"，编码层零新增设计。
2. **pipe 流不需要跨进程**：流留主进程（D1），与现状一致（现状流也留在宿主线程、脚本只见 status/headers）。语义零变化。
3. **超时从"最难"变"最简单"**：现状是"杀不掉的线程 + 循环兜底"；改造后是 `Child::kill()`，确定性更强。
4. **每请求单进程消除了并发配对问题**：IPC 严格一问一答，无多路复用、无关联 id、无乱序处理。
5. **业务逻辑零重写**：upstream/files 的沙箱策略（allowlist、Range、8 MiB 上限、路径校验、上传存储）全部原地复用，只换调用侧。

### 3.3 复杂度热点（真正要花心思的地方）

| 热点 | 等级 | 说明 |
| --- | --- | --- |
| 错误路径确定性 | 中 | OOM（分配失败 → abort/信号）、崩溃、协议中断的区分与 detail 映射；测试注入手段需设计 |
| 内存预算复测 | 中 | 64 MiB 预算下需复测：IPC 缓冲 + serde_json + 最终 runner 形态（实测基线 15.2 MiB VMS 仅含 Boa） |
| 孤儿治理 | 低-中 | 轮询线程（Unix）+ 优雅关闭兜底 kill |
| macOS 验证 | 中 | `RLIMIT_AS` EINVAL 边界（低于当前 VMS 报错）；Apple Silicon 行为需实测（见 5.3） |
| 子进程启动开销 | 低-中 | 每请求 fork/exec + Boa 初始化；mock 场景可接受，需实测基准（列为前置 spike） |

### 3.4 初步结论

- **改造量级：中等**，不是大爆炸。新增一层进程边界 + IPC，净增约 600–900 行（含测试），不动核心业务逻辑；相对核心代码（约 4260 行）增量约 15%。
- **复杂度主要不在主流程，而在边界情形**（错误映射、64 MiB 预算复测、macOS 行为、测试注入）。

## 4. 轻量替代方案检索（2026-09-27）

### 4.1 引擎级限制路线（免进程隔离）

| 路线 | 硬内存 | 硬超时 | 关键事实 | 结论 |
| --- | --- | --- | --- | --- |
| Boa 升级 | 无 | 无 | 0.22 之后无新正式版；interrupt/on_tick 相关 PR（#5380、#5475）均 closed 未合并；issue #3442（2023 起）/ #2350 仍开放；维护者承认无法提供完整资源边界（Discussion #3679、#3238）；#5367 记录过 25.7 GB 分配终止进程 | 不可行 |
| rquickjs/QuickJS-NG | 引擎 allocator 内 | 有（轮询式 interrupt） | 0.14.0（2026-09-18，MIT，活跃）；`set_memory_limit` + `set_interrupt_handler` 均为安全 API；但需 C 工具链编译 vendored QuickJS；内存限制不覆盖 Rust 侧（prelude/JSON/IPC 缓冲），custom allocator 下变 no-op；宿主桥需整体重写；偏离 ADR 0003 | 备选（代价高） |
| deno_core/V8 | 有（isolate heap limit） | 有（terminate_execution） | V8 预编译静态库约 39 MB；构建链重；rustyscript 版本落后且 timeout 不硬 | 太重，排除 |

### 4.2 现成进程管理与沙箱库

| 库 | 关键事实 | 结论 |
| --- | --- | --- |
| `process_control` 5.2.0 | MIT OR Apache-2.0、安全 API；内存限制仅 Linux/Windows（**无 macOS**）；不支持 RLIMIT_CPU；`wait()` 会吞掉 stdin（不适合 JSONL IPC）；README 警告非安全隔离机制 | 排除 |
| `process-wrap` 10.0.1 | 活跃；Unix killpg / Windows JobObject 树级强杀；无资源限制 | 可选不必要（无孙进程） |
| `shared_child` / `duct` | wait_timeout/kill/管道管理；tokio 已覆盖同等能力 | 不用 |
| `interprocess` / 帧协议库 / `postcard` / `bincode` 3.0 | 传输或序列化单点；无现成"stdio JSON 请求-响应"库；bincode 3.0 不可用 | 不用（手写 JSONL 足够） |
| `hakoniwa` / `onde-mistralrs-sandbox` / `judger` / `cageforge` / `heel` / `processkit` / `microsandbox` | 各覆盖一部分：或 LGPL（hakoniwa）、或实测缺内存限制（cageforge 0.7.1）、或不含 IPC/能力代理（全部）、或依赖 hypervisor/OCI（microsandbox）、或采用度低（heel/processkit） | 无一个满足"轻量 + 跨 Linux/macOS + 硬内存 + 硬超时 + 能力代理" |

**rlimit 基础件对比**：

- `rlimit` 0.11.0：MIT、极轻（基本只依赖 libc）、安全 `setrlimit/prlimit`、维护未停；推荐采用。
- `rustix` 1.1.5：bytecodealliance、双许可+LLVM exception、活跃；安全 `setrlimit` + Linux `prlimit`（父进程可直接给已 spawn 子进程设限）——作为备选。
- 标准库没有安全 setrlimit API；不用第三方就必须写 unsafe，与 `unsafe_code = deny` 冲突——依赖理由成立。

**总体结论**：检索范围内**不存在**"子进程 + 资源限制 + 超时 + 进程树 kill + 能力代理 + JSON IPC"的现成 Rust 库。能力代理（`ctx` 宿主函数跨边界）必然定制；这决定了任何路线都要自建这一层。

### 4.3 Wasm / VM 路线

- wasmtime 有 StoreLimits + fuel/epoch，但要把宿主 + 引擎整体编译为 wasm（WASI 无线程/进程原语，tokio/hyper 冲突面大），属架构重写级；宿主侧内存与 bulk 操作仍有抢占盲区。wasmer 同类且 interrupt 为实验性。→ 排除。
- microsandbox 等 microVM 方案依赖 hypervisor/OCI，不属轻量。→ 排除。
- 参考先例：Figma 用 QuickJS-wasm 作插件 VM 边界（浏览器环境无进程可用）；VS Code 用独立 Extension Host 进程。**业界对原生环境的通行做法就是进程隔离**。

### 4.4 最佳实践要点

- 引擎 heap limit 只在引擎原生支持时成立；permission 模型不等于资源隔离（Deno 官方立场）。
- Node `worker_threads.resourceLimits` 不覆盖 `ArrayBuffer` 等外部数据，进程级 OOM 仍可能；`node:vm` 官方明确不是安全机制。→ 印证"引擎级限制不完整"。
- 生产平台做法是组合多层防线；本方案对应为：进程边界（崩溃/资源隔离） + rlimit（硬内存） + watchdog（硬超时） + 能力代理 RPC（能力沙箱）。
- 与候选路线最接近的公开先例：Rebon-Code（Apache-2.0，Boa + 每调用 fresh 子进程 + pre_exec rlimit + watchdog 杀进程组 + Windows Job Object）。成熟度低（约 10 stars），但机制方向与本方案一致，可作为实现参考。

## 5. 对比与建议

### 5.1 路线对比

| 路线 | 硬内存 | 硬超时/崩溃隔离 | 迁移量 | 新依赖 | unsafe | 结论 |
| --- | --- | --- | --- | --- | --- | --- |
| 进程隔离（D1–D13） | 有（Linux 64 MiB RLIMIT_AS；macOS 无硬限，见 5.3 结果） | 有 | 中等（+600–900 行） | +rlimit（1 个，极轻） | 零 | **推荐** |
| 换 rquickjs | 部分（仅引擎分配） | 有（轮询中断） | 中高（宿主桥重写） | +C 工具链 + rquickjs | 零（高层 API） | 备选 |
| 升级 Boa | 无 | 无 | — | — | — | 不可行 |
| deno_core/V8 | 有 | 有 | 高（事件循环/op 模型） | V8 重依赖 | 零（高层） | 排除 |
| Wasm/VM 化 | 有（guest 内） | 有 | 架构重写 | wasmtime/microVM | 零 | 排除 |
| 现成 runner 库 | — | — | — | — | — | 不存在 |

### 5.2 建议

1. **维持进程隔离路线**（D1–D13）。它是对 issue 目标达成度最高、总代价最小的路线：不改引擎、不改宿主业务逻辑、只新增一层边界；零 unsafe（Windows 延后）；新增依赖仅 rlimit 一项。
2. **本方案不需要任何 IPC 框架或进程管理框架**：手写 JSONL（约 50 行）+ tokio 现有能力 + rlimit 封装即为最小实现；调研确认没有能显著省工的现成库。
3. **macOS 验证已确认失败（S1，2026-09-27，#89）**：macOS 降级为"进程隔离 + 超时强杀 + 无硬内存限"，与 Windows 一致；"调高默认值后重测"分支经实测排除（64 GiB 仍 `EINVAL`）。rquickjs 换引擎不再作为本期备选，只在未来需要 macOS 硬内存限时重新评估。
4. 用户约束确认：Windows 内存硬限与 unsafe FFI 不在本期范围（D7）。

### 5.3 实现前的前置验证（spike 清单）

| # | 验证项 | 目的 | 失败时的对策 |
| --- | --- | --- | --- |
| S1 | macOS（Apple Silicon）上对含 Boa 的子进程设置 `RLIMIT_AS=64 MiB` 的实测 | 确认 XNU EINVAL/预留地址空间不会让该限制失效（Rebon 先例在 macOS 跳过了 RLIMIT_AS；我们只在 Linux 实测过） | macOS 降级为无内存硬限，或调高默认值后重测 |
| S2 | 子进程启动开销基准（spawn + Boa 初始化 vs 现状线程） | 量化每请求 exec 成本；确认 mock 场景可接受 | 若不可接受，重新评估进程池（D2 升级路径） |
| S3 | 最终 runner 形态下的 64 MiB 预算复测（含 IPC、serde_json、协议缓冲） | 确认余量（当前基线仅 Boa + 简单脚本，15.2 MiB VMS） | 调高默认上限或压缩缓冲 |

**结果（2026-09-27，#89）**：一次性探针位于分支 `ci/89-process-isolation-probes`（不合并 `main`），完整命令与原始数据见 [#89 评论](https://github.com/geeknonerd/stuntdouble/issues/89#issuecomment-5856621521) 与 [CI 运行](https://github.com/geeknonerd/stuntdouble/actions/runs/36324984631)。

- **S1 → NO-GO（macOS arm64）**：GitHub `macos-15`（Apple M1 Virtual，macOS 15.7.9）上，`setrlimit(RLIMIT_AS)` 在 64 MiB、512 MiB、64 GiB 三档均为 `EINVAL`；进程启动后 VM map 约 391.6 GiB，限制必须高于当前 VM map 才被接受（512 GiB 可设但无意义）。峰值 RSS 11–12 MiB。触发降级对策：macOS 无内存硬限。
- **S2 → GO**：release 构建、各 100 次测量（开发机 Linux 与 CI `ubuntu-24.04`），spawn 往返相对进程内路径的配对延迟中位数 3.0–3.9 ms、p95 3.5–4.4 ms；占默认 `script_timeout_ms = 10000` 的 0.11% 以下。不做进程池。该数值是最终 runner 形态到来前的估计。
- **S3 → 待最终 runner 形态复测**（由收尾票 #93 执行）。

### 5.4 后续步骤（本评估通过后）

1. 起草 ADR 0014（固化 D1–D13 + 本评估结论 + S1–S3 验证义务）。
2. 更新 CONTEXT.md 术语（script worker 的进程语义）。
3. 走 `/to-spec` → `/to-tickets`（tracer-bullet 切分，含 S1–S3 spike ticket）。

截至 2026-09-27：ADR 0014 已合并（#87），CONTEXT.md 术语已更新，spec #88 与 ticket #89–#93 已发布。

> **后续修订（#91，2026-09-28）**：在最终 runner 形态上的初步实测显示，Linux 普通脚本最低约 debug 40 MiB / release 24 MiB，8 MiB `ctx.file.readText` 最低约 192 MiB（含 IPC、Boa 字符串与协议缓冲）。据此默认值调整为 256 MiB、下限调整为 64 MiB，公开契约与 ADR 0014 已同步。正式 S3 测量归档、默认值复核与 #28 验收报告由 #93 完成。本文档此前的 64 MiB 设计假设与 S1 证据保留为历史记录。

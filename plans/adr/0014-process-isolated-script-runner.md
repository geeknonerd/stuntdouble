# 进程隔离脚本执行：每请求子进程 + OS 级硬限制

- 状态：`已接受`
- 日期：2026-09-27
- 关联：[issue #28](https://github.com/geeknonerd/stuntdouble/issues/28)、[ADR 0003](0003-script-first-multi-runtime.md)（T3 与 #29 修订）、[ADR 0004](0004-host-functions-only-sandbox.md)、[方案评估](../../research/process-isolation-assessment.md)、[外部事实](../../research/process-isolation-external-facts.md)

## 背景

Boa 0.22 没有 interrupt 钩子，也没有堆内存指标或上限（ADR 0003 T3 修订），留下两个已知缺口：脚本超时后 worker 只能靠循环上限兜底回收，期间继续占槽、占内存；内存没有硬边界，只能靠并发上限缓解总量（#29 修订）。issue #28 要求用 OS 级机制把这两个缺口变成硬边界。

设计访谈确认了 13 条决策（D1–D13），轻量替代方案检索的结论是：升级 Boa 无可用 API（相关 PR 均 closed 未合并）；换引擎需要 C 工具链、重写宿主桥，且引擎级内存限制不覆盖 Rust 侧缓冲；V8/Wasm/现成沙箱库或过重、或不满足"硬内存 + 能力代理"的组合。因此选择自建轻量 supervisor 的进程隔离路线。

## 决策

### 隔离模型

- **D1 能力留在主进程**：`ctx.http`、`ctx.file`、`ctx.request.files` 的宿主实现全部留在主进程；脚本子进程只运行 Boa，通过 IPC 请求能力。能力沙箱（allowlist、文件根、上传存储）保持单点控制。
- **D2 每请求一个新子进程**：用完即退，不做进程池；请求间无共享状态由结构保证，超时或崩溃后无残留需要回收。
- **D3 self-spawn**：子进程由同一二进制的隐藏子命令启动，不属于公开 CLI 契约；单二进制分发不变。
- **D4 单轨替换**：删除进程内 `spawn_blocking` 执行路径，所有平台统一进程隔离，不提供开关或回退路径。

### 资源边界

- **D5 到期强杀**：脚本超时后由主进程立即强杀（Unix `SIGKILL`、Windows `TerminateProcess`），无宽限期；槽位即时回收。不设 `RLIMIT_CPU`，CPU 时间由该 deadline 强杀兜底。
- **D6 内存硬限**：Unix（Linux/macOS）由子进程在启动早期对自身设置 `RLIMIT_AS`（soft=hard），默认 256 MiB，可通过 `sandbox.script_memory_limit_mb` 配置，下限 64 MiB。该默认值与 floor 已按 #91 的最终 runner 实测调整（见「验证结果」）。该限制的语义是虚拟地址空间上限（非 RSS）。macOS 上该限制实测无法成立（S1，见「验证结果」），降级为不设内存硬限。
- **D12 并发上限**：保留现有 `clamp(4, 16)` 槽位公式，语义为"同时运行的脚本进程数上限"；总内存天花板 = 单进程上限 × 槽位数。

### 通信

- **D8 stdio JSON Lines**：子进程 stdin/stdout 承载协议，stdout 专用于协议，stderr 留子进程诊断；消息三类（`host_call` / `host_result` / `final_result`），严格一配一。现有宿主桥已经是 JSON 字符串形状，协议直接复用，不引入 IPC 框架。
- `ctx.http.pipe` 的流不跨进程：脚本只获得状态与 header，流由主进程持有并在脚本完成后交给客户端，与现状语义一致。

### 生命周期

- **D11 关闭协调**：首次关闭信号后停止接新连接，等待在途请求在各自 timeout 内完成；排空收尾时主进程显式杀掉仍存活的子进程；不提前杀。
- **D9 孤儿治理**：见下节。

### 孤儿治理（父进程死亡后的子进程）

Unix 上父进程死亡不会连带结束子进程（子进程会被系统收养并继续运行），所以按三层处理：

1. **正常关闭**（Ctrl-C / SIGTERM）：主进程在排空收尾时显式 kill 存活子进程。
2. **脚本超时或异常**：主进程立即强杀，槽位即时回收。
3. **主进程被 `SIGKILL`**（来不及做任何清理）：子进程内的哨兵线程每 1 秒检查一次 `std::os::unix::process::parent_id()`，一旦发现父 PID 变化（说明已被收养）就自行退出。哨兵线程独立于运行脚本的主线程，不受脚本死循环影响。

不采用 `PR_SET_PDEATHSIG`：它仅 Linux 可用且需要 unsafe。轮询使用标准库安全 API，Linux/macOS 通用；最坏情况下孤儿进程多存活约 1 秒（Linux 上另有 256 MiB 内存硬限兜底，macOS 无内存硬限）。Windows 的对应机制是 Job Object kill-on-close，随 Windows 支持阶段处理。

### 可观测性

- **D10 detail 细分**：客户端仍统一 500 `script_error`；`--verbose` 的 detail 在现有文本之外新增两条稳定文本：`"script exceeded the configured memory limit"`（内存超限）与 `"script worker terminated unexpectedly"`（异常终止）；超时保持现有文本。

### 平台范围

- **D7 Windows 延后**：本期覆盖 Linux 的完整硬边界；macOS 的内存硬限经 S1 实测失败（见「验证结果」），与 Windows 一样只获得进程隔离与超时强杀。Windows 的内存硬限（需要未封装的 Win32 FFI）延后，文档化这两个平台差异。不为 Windows 提前引入 unsafe 例外。

## 考虑过的替代方案

- **升级 Boa**：0.22 之后无新正式版；interrupt 与 heap limit 相关 PR 均 closed 未合并，上游 issue 开放多年。不可行。
- **换引擎 rquickjs/QuickJS**：有引擎级 `set_memory_limit` 与 interrupt（均为安全 API），但需要 C 工具链、重写宿主桥，且内存限制只覆盖引擎分配、不覆盖 Rust 侧缓冲——不满足硬内存的要求。保留为未来需要 macOS 硬内存限时的备选（S1 失败后已选择降级分支，见「验证结果」）。
- **deno_core/V8 与 Wasm/VM 路线**：依赖重量或迁移量远超问题本身，排除。
- **现成沙箱库**：检索范围内不存在满足"跨 Linux/macOS + 硬内存 + 硬超时 + 能力代理 + JSON IPC"的成熟库，详见方案评估第 4 节。
- **进程池复用**：省去每请求 spawn 开销，但需要污染检测、realm 重建与租借状态机；mock 场景对 spawn 开销不敏感，选择结构更简单的每请求进程。

## 实现前的验证义务

| # | 验证项 | 失败时的对策 |
| --- | --- | --- |
| S1 | macOS（Apple Silicon）上对含 Boa 的子进程设置 `RLIMIT_AS=64 MiB` 的实测 | macOS 降级为无内存硬限，或调高默认值后重测 |
| S2 | 子进程启动开销基准（spawn + Boa 初始化 vs 现状线程） | 若不可接受，重新评估进程池升级路径 |
| S3 | 最终 runner 形态下的预算复测（含 IPC、序列化与协议缓冲） | 调高默认值/floor 或压缩缓冲；#91 已据实测调整，正式证据由 #93 归档 |

## 验证结果（S1/S2，issue #89；S3 初步，issue #91）

S1、S2 已于 2026-09-27 在一次性探针上执行（分支 `ci/89-process-isolation-probes`，不合并 `main`；命令、原始数据与两处 go/no-go 见 [#89 评论](https://github.com/geeknonerd/stuntdouble/issues/89#issuecomment-5856621521)，运行记录见 [CI 运行](https://github.com/geeknonerd/stuntdouble/actions/runs/36324984631)）：

- **S1 → NO-GO（macOS arm64）**：GitHub `macos-15`（Apple M1 Virtual）上，`setrlimit(RLIMIT_AS)` 在 64 MiB、512 MiB、64 GiB 三档均以 `EINVAL` 失败——进程启动后 VM map 已约 391.6 GiB，限制必须高于当前 VM map 才被接受（512 GiB 可设，但不构成有效边界）。因此触发上表预定的降级对策：**macOS 只获得进程隔离与超时强杀，不设内存硬限**；「调高默认值后重测」分支经实测排除。峰值 RSS 11–12 MiB，说明被拒的是虚拟地址空间口径的检查，不是物理内存压力。
- **S2 → GO**：release 构建、每组 100 次测量（开发机 Linux 与 CI `ubuntu-24.04`）：spawn 往返相对进程内路径的配对延迟中位数 3.0–3.9 ms、p95 3.5–4.4 ms，占默认 `script_timeout_ms = 10000` 的 0.11% 以下，不触发进程池重评估；该数值是最终 runner 形态到来前的估计。
- **S3 初步测量（#91，2026-09-28）**：Linux 本机 debug/release 下，普通脚本最低约 40/24 MiB，8 MiB `ctx.file.readText` 最低约 192 MiB（含 IPC、Boa 字符串与协议缓冲）。据此把默认值调整为 256 MiB、floor 调整为 64 MiB；正式测量归档、默认值复核与 #28 验收报告由 #93 补齐。

本结果是既定降级分支的落地记录，D1–D13 的决策本身不变；D6/D7 的平台口径与备选说明按此结果收敛。评估文档的对应更新见 [方案评估](../../research/process-isolation-assessment.md) §5.3。

## 后果

**正面**：超时从"杀不掉的线程"变为确定性强杀；内存获得 OS 级硬限（Linux；macOS 见「验证结果」）；崩溃隔离由进程边界保证；`ctx` 契约与脚本源码不变；upstream/files 策略逻辑零重写，改造集中在桥的调用侧。

**负面（接受）**：每请求增加一次子进程启动开销（S2 量化）；`RLIMIT_AS` 限制的是虚拟地址空间而非 RSS；macOS 无法设置有效硬内存限（S1 实测 `EINVAL`），与 Windows 一样只有进程隔离与超时强杀；新增一个极轻依赖（`rlimit`，MIT）用于安全设置 rlimit。

**中性**：执行单元从线程变为进程，"script worker"的领域含义同步更新到 CONTEXT.md。

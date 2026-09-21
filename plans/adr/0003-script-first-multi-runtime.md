# 第一版采用完全脚本化，支持 JavaScript/TypeScript + Python 运行时

- 状态：`已接受（运行时路径待定）`
- 日期：2026-09-18
- 关联：[ADR 0001](0001-no-shared-state-in-v1.md)、[ADR 0002](0002-route-model-only-in-v1.md)、[架构设计最佳实践调研](../../research/architecture-best-practices.md)

## 背景

讨论过三条变换表达力路线：纯声明式模板、声明式 + 脚本逃逸口、完全脚本化。

已验证场景（设备文档）的 transform 有两种复杂度：CSV 生成（数组遍历拼接，属声明式）；PDF fetch（外部 HTTP + 二进制流，需编程能力）。既有实现已经是"每个接口一段 JS"的完全脚本化形态。

本 ADR 记录脚本路线与语言选择，**前提是 mock server 主体用 Rust 实现**。

## 决策依据

1. 真实场景全部涉及外部网络请求或二进制处理，纯声明式覆盖不了。
2. Rust 主体意味着脚本能力是"嵌入外部运行时"，模板 DSL 与脚本是两条不同的工程投入；脚本能一次性覆盖 transform 全部复杂度。
3. 目标用户是开发者，写 JS/TS/Python 的门槛低于学一套私有模板 DSL。

## 决策

1. **transform 层完全脚本化**，不做私有模板 DSL。
2. **实现语言 Rust；脚本运行时第一版支持 JS/TS + Python。**
3. **运行时全部内置（in-process），零外部环境依赖**（无需安装 Node.js / CPython）：
   - **JS/TS：Boa**（纯 Rust ECMAScript 引擎，boa_engine 0.22.x，官方自述覆盖 90%+ 最新 ECMAScript 规范）+ **boa_runtime**（启用 `fetch` / `interval` / `url` 等 WebAPI 扩展；fetch 由 `BlockingReqwestFetcher` 提供）。
   - 版本选择规则：Rust 工具链跟随 stable；MSRV 取“安全门槛 + 依赖树”共同确定的实际最低值，而不是单纯最小化。0.20 / 0.21 曾因 MSRV 更低（1.82 / 1.88）被评估，但其依赖链仍使用已归档的 `paste`（RUSTSEC-2024-0436），且压低 MSRV 必须把 `time` 锁在受 RUSTSEC-2026-0009 影响的版本，`cargo deny check advisories` 无法通过；0.22 改用 `pastey` 并可搭配已修复的 `time 0.3.47+`。因此选择 0.22.x，MSRV 为 1.91。Boa 升级必须在 PR 中重新校验 MSRV，并同步 `Cargo.toml` 与本文档。
   - **Python：RustPython**（纯 Rust Python 解释器，CPython 3.14 兼容子集）。接受不支持 C 扩展（无 numpy/pandas），标准库核心模块够用。
   - **TypeScript：第一版不支持**。Boa 只执行 ECMAScript，运行 `.ts` 必须先做类型剥离；第一版不引入 swc_core / oxc_transformer。需要 TS 的用户自行用 `tsc`/esbuild 编译出 `.js` 交给产品，产品不做任何转译。产品仍可发布脚本 API 的 `.d.ts` 类型定义（纯文档产物，零运行时成本），供编辑器补全使用。
4. **脚本生态边界**：第一版不支持 `import`/`require` 第三方包（无 npm/pip）；仅内置少量常用库（如 Python `requests` 语义的 HTTP 封装、`csv`、`json`）。
5. **安全边界由 Rust 宿主层实现**（脚本无法绕过）：
   - 网络：脚本调用宿主提供的受限 HTTP 函数，Rust 层校验白名单域名；不向脚本暴露裸 `fetch`。
   - 文件：仅允许预置目录（如 `__files/`），拒绝绝对路径与目录穿越。
   - 超时：脚本执行限时（默认 10 秒，可配置），超时终止。
   - 内存：设置堆上限（默认 ~64MB），超限报错。
   - 错误：异常堆栈只进服务端日志，客户端只收错误码。

## 风险

- **Boa 官方自述为 experimental**（仓库 README 原话："Boa is an experimental JavaScript lexer, parser and interpreter written in Rust"），conformance 90%+（test262 结果见 boajs.dev/conformance）。缓解：脚本 API 保持小面积（不依赖冷门语法）；错误统一映射为 500 + 服务端日志；锁定 boa_engine/boa_runtime 版本并在升级时跑回归。
- **RustPython 不兼容 C 扩展**（无 numpy/pandas）；标准库为子集。缓解：文档明确支持清单，需求外场景引导到 JS 侧。
- **嵌入式引擎没有 npm/pip 生态**：与主流 mock 工具的脚本能力相比是显性退化。缓解：内置 HTTP/CSV/JSON 常用能力覆盖已验证场景；不承诺动态包安装。
- **单进程执行的爆炸半径**：脚本 panic 若未隔离会波及主服务。缓解：脚本执行捕获 panic（`catch_unwind`）+ 超时终止；必要时升级为进程隔离（保留同一 API 契约）。

## 后果（待逐条确认）

**正面**：表达力无上限；与既有 json-server middleware 实践连续；Rust 主体保证单二进制与性能。

**负面（接受）**：失去声明式配置的可读性与静态校验；脚本安全成为核心风险面；引入外部运行时的分发难题。

## JS 与 TS 的判断

**结论：TypeScript 为主，JavaScript 天然兼容。**

1. TS 是 JS 的超集，能跑 TS 的运行时去掉类型注解即可跑 JS，支持 TS 等于免费支持 JS；反之不成立。
2. 本产品需要一套脚本 API（请求上下文、上游 fetch、响应构造、文件访问）。类型定义文件（.d.ts）是最好的 API 文档——编辑器自动补全，用户少犯错。这是开发者工具的产品级收益。
3. 运行时已原生支持：Node.js 自 v22.18/v23.6 默认启用类型剥离，v24.12/v25.2 起稳定；Deno 原生运行 TS。无需构建步骤。
4. 代价：类型剥离不做类型检查（检查由编辑器/tsc 承担）；enums/namespaces 等需要完整转译的语法罕见，文档标注即可。

**约束**：产品分发脚本 API 的类型定义；默认模板生成 .ts，同时接受 .js。

## 待决策项

- 脚本运行时的分发形态：依赖用户环境 / 随产品分发 / 容器镜像
- 沙箱安全边界（网络白名单、文件访问、超时与内存上限）
- 脚本 API 契约（脚本能读什么、能做什么、怎么返回响应）
- 子进程 vs 嵌入式引擎的最终技术路径

## 修订（T3，#6）：Boa 0.22 下可执行的安全边界

T3 落地沙箱加固时核实：Boa 0.22 没有 interrupt 钩子，也没有堆内存指标或上限；能限制执行的上限只有循环次数、递归深度与 VM 栈大小（`RuntimeLimits` 另有一个只影响异常回执条数的 `backtrace_limit`）。决策第 5 条中"超时终止"与"内存堆上限（~64MB）"两条因此按如下方式收敛：

- **超时**：`sandbox.script_timeout_ms`（默认 10000）是客户端应答时限。到点后宿主立即回 500 `script_error`，并把该 worker 线程留在后台；Boa 没有中断钩子，`spawn_blocking` 也无法取消，worker 只能由循环次数上限（100,000,000）兜底回收。超时后继续分配内存的脚本仍可能拖慢进程，这是当前接受的风险。
- **内存**：Boa 0.22 无堆观测点，64MB 堆上限与"超限报错"无法实现。可执行的替代边界是：`ctx.http.get` 响应体 8 MiB 上限、循环次数上限、递归 512 层、VM 栈 10,240 项、每请求独立 worker 线程。
- **panic**：引擎 panic 由 `catch_unwind` 捕获，worker 线程 panic 由 `JoinError` 映射，两者都返回 500 `script_error`；每个请求使用独立的 `Context`，单路由失败不跨请求传播。

升级路径：Boa 暴露 interrupt 钩子或堆指标后接入，即可恢复"超时终止"与内存上限；若要把内存变成硬边界，需要把脚本执行改为子进程隔离（ctx 契约不变），该取舍不在 T3 范围内，另立 ADR。`docs/contracts/ctx-api.md` 的措辞由 T8（#11）统一收敛。

## 替代方案

- 纯声明式模板 + 脚本逃逸口（拒绝：私有 DSL 设计成本高，且已验证场景主体落在脚本侧）
- 只支持 JS/TS 不支持 Python（待定：可能仍是正确的 v1 选择）

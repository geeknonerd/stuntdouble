# Mock Server 产品功能定义

- 状态：`功能已收敛`（配置文件主格式已确认，公开契约已在 T8 冻结）
- 更新：2026-09-22
- 关联：[Mock 服务选型调研](../research/mock-server-landscape.md)、[示例文档清单与二进制下载场景](demo-document-catalog.md)、[配置契约](../docs/contracts/config.md)、[ctx API 契约](../docs/contracts/ctx-api.md)、[CLI 契约](../docs/contracts/cli.md)

## 1. 产品命题与定位

一款用 Rust 实现的 Mock Server 产品 **Stunt Double (`stuntdouble`)**：以声明式路由配置描述接口，用 JavaScript / Python 脚本（运行时内置）完成从数据源取数、调外部上游、读写文件的响应构造；把"读数据源（本地 JSON/外部 HTTP）、调用外部网络、返回二进制/文件"当作一等能力，替代 json-server + 自写 middleware 的组合。请求间共享状态类能力不在第一版（v2/v3 考虑 SQLite）。

**定位（主）**：面向需要对接真实外部依赖的后端与集成开发者的联调假服务，用于本地与 CI。
**顺风加成**：AI 编码代理的测试后端（零运行时依赖、确定性、二进制文件能力天然契合）。
**不做**：不与通用静态 stub 方案（Postman / Mockoon / Prism）正面竞争；通用能力只做兼容，不做专门投入。详见 [ADR 0006](adr/0006-product-positioning.md)。

## 2. 第一版已确认范围（2026-09-18 收敛）

| 维度 | 结论 | 依据 |
| --- | --- | --- |
| 产品形态 | 可对外发布的 Mock Server 产品，非内部工具 | 本轮讨论 |
| 实现语言 | Rust（产品主体） | 本轮讨论 |
| 脚本运行时 | **全部内置**：Boa（JS, v0.22.x）+ RustPython（Python stdlib 子集），零外部环境依赖（无需安装 Node/Python） | [ADR 0003](adr/0003-script-first-multi-runtime.md)、[脚本运行时选型调研](../research/script-runtime-selection.md) |
| TypeScript | **第一版不支持**（不引入 swc/oxc 转译）；需 TS 者自行编译为 `.js`。产品仍发布 `.d.ts` 供编辑器使用 | [ADR 0003](adr/0003-script-first-multi-runtime.md) |
| 脚本能力供给方式 | 半托管：外部能力一律经宿主函数注入；不向脚本暴露引擎自带的 `fetch`/`fs`/`os`/`subprocess` | [ADR 0004](adr/0004-host-functions-only-sandbox.md) |
| 脚本生态边界 | **不支持 import/npm/pip**；仅内置少量常用库（如受限的 http.get/post、csv、json、text 处理） | ADR 0003 |
| 配置生效方式 | 静态配置：改配置文件 + 重启（模式 1）；预留热重载（模式 2）与管理 API（模式 3）的扩展入口 | [架构设计最佳实践调研](../research/architecture-best-practices.md) |
| 配置模型 | 路由模型：接口逐条声明（匹配 + 数据源 + 变换 + 响应四段流水线） | [ADR 0002](adr/0002-route-model-only-in-v1.md) |
| 数据源 | 本地静态数据文件、外部上游接口 | [ADR 0001](adr/0001-no-shared-state-in-v1.md) |
| 共享状态 | 不支持；v2/v3 考虑基于 SQLite 的持久化 | [ADR 0001](adr/0001-no-shared-state-in-v1.md) |
| 资源派生模型 | 不实装；未来可作为"OpenAPI 预设生成器"实现，但不作为独立引擎 | [ADR 0002](adr/0002-route-model-only-in-v1.md) |
| 变换表达力边界 | 完全脚本化，不做私有模板 DSL；脚本语言为 JS（第一版）+ Python | ADR 0003 |
| 文件 I/O 语义 | 静态文件目录为唯一文件根：配置声明，脚本文件操作只能在该目录内，相对路径默认解析到此根；上传由宿主解析 multipart 并落到系统临时目录下的每请求独立随机子目录（Unix 0700，脚本不可见路径），请求结束清理；响应侧支持流式透传与本地文件流，Range 透传/支持；纯内存响应不支持 Range；上传上限默认 20MB（可配置） | 本轮讨论 |
| 上游失败语义 | 以"是否拿到 HTTP 响应"为唯一分界：有响应则视为数据、默认透传状态码（脚本可改写）；传输层失败抛异常，未捕获返回 502 + `request_id`；脚本异常返回 500；脚本未调用 `respond` 视为逻辑错误 | [ADR 0005](adr/0005-upstream-failure-semantics.md) |
| 超时与重试 | 脚本总超时默认 10 秒（可配置），上游超时 = `min(剩余脚本时间, opts.timeout_ms)`，并预留最多 100ms 的回复余量以避免与脚本硬超时竞态；默认不重试，`opts.retries` 显式开启且上限 3 次，只对传输层失败生效 | [ADR 0005](adr/0005-upstream-failure-semantics.md) |
| 可观测性 | 每请求一条结构化日志（request_id/路由/耗时/上游链/状态码/错误分类）；默认不记录请求体与响应体；诊断开关附加 `detail`；堆栈永不进响应 | [ADR 0005](adr/0005-upstream-failure-semantics.md) |
| 响应推进 | **不进第一版**，列入不做清单；v2 可加声明式响应序列或脚本 `callCount` 数字 | 本轮讨论 |
| 目标平台与分发 | 核心二进制：Linux x86_64、macOS arm64、Windows x86_64；分发：GitHub Releases 二进制 + 容器镜像（`linux/amd64`、`linux/arm64`）；GUI（如未来做）优先 Linux + macOS | 本轮讨论 |
| 环境变量注入 | 支持 `ctx.env.*`，来源为 `.env` 或系统环境变量 | 本轮讨论 |

## 3. 演进约束（第一版 → 模式 2/3 的设计边界）

- **单一执行模型**：所有来源最终都编译为 `match → source → transform → response` 流水线；不得出现第二个执行路径。
- **配置即事实源**：第一版以文件系统为唯一 SOT；若引入管理 API，应提供 `config save` 将运行态落盘并明确冲突策略。
- **reload 入口统一**：热重载与管理 API 都应复用同一个 `reload()` 入口实现原子切换路由表。
- **控制面隔离**：若暴露 Admin API，应使用独立端口或 `localhost`，并实施基础鉴权。
- **脚本沙箱强制**：脚本能力默认开放，但必须有硬限制：超时（默认 10 秒，可配置）、循环/递归/栈上限、网络白名单、文件目录限制、错误不向客户端回显堆栈。内存硬上限（~64MB）在 Boa 0.22 下无观测点、进程内无法执行，降级为文档化风险（见 [ADR 0003](adr/0003-script-first-multi-runtime.md) T3 修订）；硬隔离列入后续切片。
- **OpenAPI 预设生成器是"批量生成普通路由"**，不是新增一个资源派生引擎。

## 4. 脚本 API 契约（半托管，已确认）

脚本入口是一段 JS 或 Python 源码，宿主注入单一对象 `ctx`。

| 分组 | API | 约束 |
| --- | --- | --- |
| 请求只读 | `ctx.request`: `method` / `path` / `params` / `query` / `headers` / `bodyText` / `bodyBytes` | 只读；不暴露原始 socket |
| 外部取数 | `ctx.http.get(url, opts)` / `ctx.http.request(method, url, opts)` | 仅白名单 host；`ctx.http.get` 于 T4 实现（`timeout_ms`；`retries` / `backoff` 后续切片），`ctx.http.request` 后续切片；返回 `{status, headers, text(), bytes()}` |
| 二进制透传 | `ctx.http.pipe(url, {status, headers})` | 于 T6 实现；上游 2xx 响应体直接流到客户端，不进脚本堆内存；默认透传上游 2xx 状态并转发 Range/206；不做字节级变换 |
| 读文件 | `ctx.file.readText(p)` / `ctx.file.readBytes(p)` / `ctx.file.stream(p)` | 只读；路径相对静态文件目录解析；拒绝绝对路径与 `..`；v1 不提供脚本写文件能力（上传由宿主落盘） |
| 响应 | `ctx.respond(status, headers, body)`；body 为 string / bytes / 文件流引用 | 文件流引用支持 Range；纯内存 body 不支持 Range；不调用则视为未产生响应（固定错误码，不静默 200） |
| 请求内暂存 | `ctx.local`（键值，随请求销毁） | **不跨请求**（ADR 0001）；与"共享状态"严格区分 |
| 日志 | `ctx.log.info/warn/error` | 只进服务端日志，绝不出现在响应里 |
| 定时 | `setTimeout` / `setInterval`（boa_runtime） | 受整体脚本超时截断 |
| 不可用 | `require` / `import` / `fetch` / `fs` / `process` / Python `os`、`subprocess`、`socket` | 不注入即不可见 |

## 5. 明确不做清单（第一版）

| 不做项 | 说明 | 依据 |
| --- | --- | --- |
| 请求间共享状态 | 凭证签发/校验、POST 后 GET 读回、依赖前次请求的分支响应 | [ADR 0001](adr/0001-no-shared-state-in-v1.md) |
| 响应推进 | 按调用次序变化的轮询状态机；v2 候选 | 本轮讨论 |
| 资源模型自动 CRUD | json-server 式自动路由；v2 候选（作为路由模型的预设生成器） | [ADR 0002](adr/0002-route-model-only-in-v1.md) |
| TypeScript 转译 | 不引入 swc/oxc；用户自行用 tsc/esbuild 编译为 `.js` | [ADR 0003](adr/0003-script-first-multi-runtime.md) |
| 第三方包 | 不支持 import / npm / pip | ADR 0003 |
| 脚本写文件 | 不提供 `ctx.file.write`；上传由宿主落盘 | 本轮讨论 |
| 上游自动重试 | 默认不重试；`opts.retries` 显式开启 | [ADR 0005](adr/0005-upstream-failure-semantics.md) |
| 热重载与管理 API | v1 只做"改配置文件 + 重启"（模式 1） | [架构设计最佳实践调研](../research/architecture-best-practices.md) |
| GUI / 桌面端 | v1 不做；如未来做，优先 Linux + macOS | 本轮讨论 |
| HTTP 之外的协议 | 不支持 WebSocket / GraphQL / gRPC；v1 仅 HTTP/1.1 | 本轮讨论 |
| 内置 TLS | 不内置服务端证书；用反向代理（nginx/Caddy）承载 HTTPS；v2 候选 `--tls-cert/key` | 本轮讨论 |
| 多实例协调 | 单进程单实例；多实例 = 多二进制多端口，无跨实例协调 | 本轮讨论 |
| 引擎级上游鉴权配置 | 上游认证由脚本自行携带 header，不在引擎层做鉴权配置 | 本轮讨论 |

## 6. 待定

- 实际 MSRV：Rust 1.91（由 Boa 0.22 决定；clap 4.6 与 toml 1.x 要求 1.85）。策略是工具链跟随 stable、MSRV 取“安全门槛 + 依赖树”共同确定的实际最低值：不允许为压低 MSRV 保留未修复的 advisory 或未维护依赖。RustPython 接入后需重新校准。
- v2 路线：SQLite 共享状态（ADR 0001 预留）、声明式响应序列、资源模型预设生成器、Admin API（模式 3）、内置 TLS。
- crates.io 包名保留与首个发布凭据配置。
- 独立治理邮箱；当前 Code of Conduct 使用 GitHub 私密报告。
- 自定义域名；当前使用 GitHub Pages。

## 7. 术语

领域词汇统一维护在仓库根目录 [CONTEXT.md](../CONTEXT.md)。

## 8. 开发与发布规范

Git 工作流、版本号、发布策略、CI 门槛与供应链规范见 [docs/development.md](../docs/development.md) 与 ADR 0010–0012。

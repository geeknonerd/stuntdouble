# Post-v1 开发路线：OAuth authorization mock

- 状态：`提议中`
- 日期：2026-10-22
- 驱动场景：[OAuth authorization mock](oauth-authorization-mock.md)
- 关联：[ADR 0001](adr/0001-no-shared-state-in-v1.md)、[ADR 0009](adr/0009-open-core-and-funding.md)、[ADR 0011](adr/0011-contract-compatibility.md)、[ADR 0015](adr/0015-python-runtime-deferred-to-v2.md)、[ADR 0016](adr/0016-v1-ctx-pending-capabilities-deferred.md)、[ADR 0019](adr/0019-host-crypto-api.md)、[ADR 0020](adr/0020-fallback-route-and-method-mismatch.md)、[ADR 0021](adr/0021-v2-shared-state.md)、[产品功能定义](product-definition.md)、[`ctx` API 契约](../docs/contracts/ctx-api.md)

## 目标

以 OAuth authorization mock 的 A01–A20 与第 13 节验收清单作为首个真实场景，规划一条不引入第二套执行路径的开发路线。

这条路线同时验证三件事：

1. Stunt Double 能否覆盖需要跨请求状态的真实鉴权链路。
2. 新增能力是否仍以通用 host function 供脚本使用，而不是为 OAuth 写专用引擎。
3. 每个阶段是否都能独立发布、测试和回退。

## 已确认决策

1. **硬验收**：A01–A20 与第 13 节必须全部通过；不接受只实现无状态子集后宣称完成。
2. **通用能力**：OAuth 逻辑留在脚本中；宿主只补通用能力，工作名称为 `ctx.state` 与 `ctx.crypto`。
3. **v2 边界**：该场景触发 v2 的 Shared state；Python 仍是独立的 v2 候选，不随本路线进入实现。
4. **术语**：场景统一称为 **OAuth authorization mock**，不再使用“OAuth 适配器”或“适配层”。

## 能力切分

| 能力 | 当前状态 | 所属阶段 |
| --- | --- | --- |
| 显式 Route、`ctx.respond`、302、JSON、header | 已支持 | 直接使用 |
| `ctx.request.bodyBytes`、原始查询串 | 已实现（M1.1，#171：`bodyBytes` 与 `rawQuery`） | v1.x |
| 安全随机、SHA-256、HMAC-SHA256、常量时间比较 | 未实现；脚本层不能可靠提供安全语义 | v1.x |
| 未知路径与错误方法的可控响应 | 未知路径已支持通过 [fallback] 脚本响应（#173）；错误方法留待 #174 | v1.x |
| 授权码生命周期、access token 生命周期、签名 request ID 防重放 | 需要跨请求数据，v1 明确不提供 | v2 |
| OAuth 授权、令牌与 userinfo 流程 | 需要以上能力组合 | 场景验收 |

## 路线

### M0：方向固化与契约收敛

**目的**：先把难以逆转的决策写成 ADR，再进入代码。

**状态**：已完成（2026-10-22）。

**交付**

- 场景契约已收编为 [OAuth authorization mock](oauth-authorization-mock.md)；M3 再新增对应的可运行演示目录。
- [ADR 0019](adr/0019-host-crypto-api.md) 已接受：Host crypto API 的边界、算法、输入输出与失败语义。
- [ADR 0020](adr/0020-fallback-route-and-method-mismatch.md) 已接受：Fallback Route 与方法不匹配的响应模型，保持单一执行模型。
- [ADR 0021](adr/0021-v2-shared-state.md) 已接受：v2 Shared state 的生命周期、原子操作、TTL、容量与重启语义。
- 收敛场景契约中的两处歧义：302 响应的 `Content-Type`，以及内部错误使用 `server_error` 还是 `script_error`。
- 更新 `CONTEXT.md`、产品功能定义与文档索引。`docs/contracts/` 与类型定义只描述已实现能力，因此随对应代码落地同步，不在 M0 提前承诺。

**退出条件**

- 三份 ADR 已接受。
- 契约不再依赖未定义的“适配层”或未说明的错误体字段。
- 不产生运行时实现改动。

### M1：v1.x 通用底座

**目的**：先用兼容 `apiVersion` 1 的增量能力覆盖所有无状态部分。

**M1.1 请求快照保真（已完成，#171）**

- 新增 `ctx.request.bodyBytes`（精确请求 Body 字节，`Uint8Array`；未读取为 `null`）与 `ctx.request.rawQuery`（原始 query 片段，保留重复参数、顺序与编码）。
- 保持现有 `bodyText` 与已解码 `query` 的行为不变。
- 已用签名规范化、原始 Body 哈希（2048 零字节的 SHA-256，非 FIPS 180-4 官方向量）和重复查询参数端到端测试验证。
- tradeoff: 快照保真 e2e 测试逐例手写 config 与 fixture（3 例），与既有测试风格一致；业内经验以 4 例以上近似重复变体为提取 helper 门槛。升级触发：第 4 个快照测试落地时提取 `snapshot_config(route, path)` 小 helper。

**M1.2 Host crypto API**

- 按 [ADR 0019](adr/0019-host-crypto-api.md) 提供 `randomBytes`、`sha256`、`hmacSha256` 和 `timingSafeEqual`。
- 使用 RustCrypto 与 OS CSPRNG，不提供 JavaScript 或 `Math.random()` 降级路径。
- 固定输入长度上限、返回类型、`crypto_invalid_input` / `crypto_random_unavailable` 错误码和日志脱敏要求。
- 使用公开测试向量做已知答案测试，并覆盖非法长度与空输入。
- 不把密钥、请求 Body 或签名原文写入日志。

**M1.3 Fallback Route 与错误响应（not_found 已完成 #173，method_not_allowed 留待 #174）**

- 按 [ADR 0020](adr/0020-fallback-route-and-method-mismatch.md) 增加可选的 `[fallback]` Route 与只读 `ctx.match`。（已完成 #173：支持可选 `[fallback]` 表与只读 `ctx.match.kind = "not_found"`）
- 在 Body 解析前区分 `not_found` 与 `method_not_allowed`，使脚本返回精确 JSON；405 的 `Allow` 由宿主注入。（`not_found` 早期判定已完成 #173；`method_not_allowed` 与 `Allow` 注入留待 #174）
- 未配置 `[fallback]` 时保持现有 404 行为；不引入通配方法或通配路径。（已完成 #173）
- 保持路由声明、脚本 worker、`ctx.respond` 和日志链路不变，Fallback 禁止流式 Body。（已完成 #173）
- 用未知路径、方法错误和错误优先级测试验证。（未知路径与 fail-closed 验证已完成 #173；方法错误留待 #174）

**退出条件**

- 无状态验收子集通过。
- `ctx` 契约、类型定义和文档同步。
- `cargo fmt`、`clippy`、`test`、`doc`、`deny`、`audit` 全部通过。
- 可以独立发布一个 v1.x 版本，而不依赖 Shared state。

### M2：v2 Shared state

**目的**：为 OAuth 的一次性授权码、access token 与签名防重放提供宿主状态。

**状态**：原型已完成（#175，2026-10-22），正式实现已完成（#176）。

**已完成原型**

- 已在独立原型中验证 worker 进程到宿主状态存储的桥接。
- 并发发起两次相同 code 的兑换，已确认只有一个成功。
- 已覆盖 TTL、进程重启清空和状态容量上限。
- 原型只回答状态接口与并发语义，未并入产品代码；结论见 [spike README](../research/spikes/175-shared-state-ipc/README.md)。

**正式实现（#176，已完成）**

- 按 [ADR 0021](adr/0021-v2-shared-state.md) 增加进程内、重启即清空的 Shared state。
- 提供 `get`、`set`、`take`、`setIfAbsent`，支持按 key 的固定 TTL；只接受有损转换可检测的 JSON 值。
- 所有状态操作由宿主执行；脚本闭包、worker 变量不得保存跨请求事实。
- 固定 1024 项、单项 16 KiB、总量 8 MiB 的边界；过期项惰性清理，容量满时 fail closed，不做静默淘汰。
- `take` 与 `setIfAbsent` 由宿主保证原子性；状态写入不随请求失败回滚。
- 不提供持久化、跨实例协调或 Admin API。
- 用并发兑换、重复 request ID、过期、重启和容量测试验证。

**退出条件**

- `ctx.state` 的公开契约与类型定义完成。
- ADR 0001 的 v2 修订与 ADR 0021 的实现、测试一致。
- 单独发布状态能力时，OAuth 场景尚未完成也能被明确标记为未验收。

### M3：OAuth authorization mock 场景

**目的**：用真实脚本组合 M1 与 M2 的能力，完成 A01–A20。

**交付**

- 新增 `demo/oauth-authorization-mock/`，包含 TOML、脚本、固定 fixture 与运行说明。
- 实现 authorize、token、userinfo 三条 Route。
- 实现 code 一次性、60 秒过期，access token 300 秒过期，签名 request ID 10 分钟防重放。
- 实现 404 `not_found`、405 `invalid_method` 与 `Allow`，错误体只包含契约字段。
- 增加端到端测试，覆盖 A01–A20、错误优先级、日志脱敏和离线约束。
- 验证不访问外部网络、不读取真实密钥、不持久化 code 或 token。

**退出条件**

- A01–A20 与第 13 节全部通过。
- 场景在 Linux、macOS、Windows 测试矩阵中可运行。
- 演示不依赖外部服务或额外运行时。

### M4：公开路线图与发布

**目的**：把已实现的能力与未承诺方向公开，避免候选列表被误读为承诺。

**交付**

- 发布 `Now / Next / Later` 路线图：OAuth authorization mock 在 `Now`，Shared state 在 `Now/Next`，Python 明确留在 `Later`。
- 更新中文与英文文档索引、`CHANGELOG.md`、`types/ctx-api-v1.d.ts` 和发布说明。
- 按 ADR 0009 区分开源核心工作与未来商业工作，不承诺日期或商业化功能。

**退出条件**

- 公开路线图可独立阅读。
- 文档链接与双语配对检查通过。
- 发布后复核场景和契约文档。

## 依赖与并行度

- M0 是所有后续里程碑的前置。
- M1.1、M1.2、M1.3 可在 M0 完成后并行。
- M2 原型与正式实现均已完成（#175、#176）。
- M3 依赖 M1 全部完成与 M2 发布。
- M4 依赖 M3 验收完成。

## 明确不做

- 不为 OAuth 增加专用引擎或第二套执行路径。
- 不把 Shared state 放进脚本闭包、worker 变量或持久化文件。
- 不在本路线中实现 Python 运行时。
- 不在本路线中实现 SQLite、Admin API、热重载、定时器或响应序列。
- 不通过外部 sidecar、真实身份系统或上游网络补齐状态。

## 待收敛的风险

- 原始查询串的表示需要保留重复参数与编码顺序，不能退回当前的最后值 map。
- M1.1 的 `bodyBytes` 必须为未读取 Body 的 Fallback 提供 `null`，不能与真正的空 Body 混淆。
- 该场景目前是自测驱动场景，后续仍需真实用户反馈来决定是否扩展到其他鉴权流程。

## 完成定义

当且仅当以下条件全部满足，OAuth authorization mock 才算完成：

- A01–A20 与第 13 节全部通过。
- 所有响应、错误码、`Allow` 和头部符合契约。
- 授权码只能兑换一次，过期、重放与令牌生命周期可验证。
- 日志不包含 `client_secret`、code、access token 或签名密钥。
- 服务不访问外部网络，不持久化状态。
- 文档、类型定义、发布说明与公开路线图同步。

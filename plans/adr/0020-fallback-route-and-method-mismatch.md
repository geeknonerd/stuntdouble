# Fallback Route：可脚本化的 404 与 405 错误响应

- 状态：`已接受`
- 日期：2026-10-22
- 关联：[ADR 0002](0002-route-model-only-in-v1.md)、[ADR 0004](0004-host-functions-only-sandbox.md)、[ADR 0011](0011-contract-compatibility.md)、[ADR 0019](0019-host-crypto-api.md)、[Post-v1 开发路线](../post-v1-roadmap.md)、[OAuth authorization mock](../oauth-authorization-mock.md)

## 背景

当前匹配器只返回第一个 `method + path` 都命中的 Route。没有命中时，服务器统一返回 404 `not_found`；因此即使路径存在、只是方法错误，客户端仍收到 404，且默认错误体无法由脚本改成应用所需的 JSON。

OAuth authorization mock 要求未知路径返回 404 `not_found`，已知路径的错误方法返回 405 `invalid_method` 并携带 `Allow`。若直接在引擎里写死 OAuth 错误体，或在 Route 之外增加错误响应配置，就会形成第二条响应路径；若用通配 `method = "*"` 的 Route，则会把正常匹配、错误分类和声明顺序混成一套难以解释的规则。

## 决策

### 配置与优先级

新增一个可选的全局 Fallback Route：

```toml
[fallback]
name = "http-errors"                        # 可选
script = "scripts/http-errors.js"
```

- `[fallback]` 是单个表，不属于 `routes` 数组，不参与普通 Route 的声明顺序竞争。
- `routes` 仍必须非空。Fallback Route 只负责错误响应，不能替代第一个正常 Route。
- `name` 规则与 Route 相同；`script` 必须通过现有扩展名校验。相对路径按配置文件目录解析。
- 未配置 `[fallback]` 时，未知路径和方法错误的现有 404 行为完全不变。这使配置变更保持增量兼容，并避免已有测试与部署突然改为 405。
- 禁止在普通 Route 中使用通配方法或通配路径；本决策不引入 `*` 语法。

每次请求按以下固定顺序分类：

1. 按声明顺序查找第一个 `method + path` 都命中的普通 Route；命中后进入原有流水线。
2. 若没有普通 Route 命中，但至少有一个 Route 的 path 匹配，则分类为 `method_not_allowed`，并收集这些 Route 的方法。
3. 若 path 也没有命中且配置了 Fallback Route，则分类为 `not_found` 并运行 Fallback Route。
4. 若 path 没有命中且未配置 Fallback Route，则保持现有引擎 404 `not_found`。

Fallback Route 始终低于所有普通 Route，因此配置顺序不会改变正常 Route 的“声明顺序优先”规则。

### Fallback 上下文字段

`ctx.match` 对所有脚本只读可用：

```ts
type SdMatchKind = "route" | "not_found" | "method_not_allowed";

interface SdMatch {
  readonly kind: SdMatchKind;
  readonly allowedMethods: readonly string[];
}
```

- 普通 Route：`kind = "route"`，`allowedMethods = []`。
- 未知路径：`kind = "not_found"`，`allowedMethods = []`。
- 方法错误：`kind = "method_not_allowed"`；`allowedMethods` 是所有 path 匹配 Route 的去重方法列表，按首次声明顺序排列，统一大写。
- Fallback 的 `ctx.request.params` 为空对象，因为方法错误可能同时匹配多个不同参数模式，没有唯一参数集。

### 请求 Body 与执行顺序

Fallback Route 是错误响应器，不是通用通配 Route：

- 运行 Fallback 前不读取或解析请求 Body；错误分类必须优先于 Body 大小、multipart 解析和 Body 超时错误。
- Fallback 快照的 `bodyText` 为 `null`，`files` 为 `[]`。M1.1 新增 `bodyBytes` 后，Fallback 中该字段为 `null`，表示“未读取”，不得用空数组伪装成空 Body。
- Fallback 仍通过同一个 Route 流水线、同一个脚本 worker、同一个并发槽与同一套宿主能力执行；它不绕过 worker 隔离、超时、内存限制或日志。
- Fallback 只能产生 buffered `ctx.respond` 响应，不允许 `ctx.http.pipe`、文件流或上传流作为响应 Body。错误响应不得以流式方式在状态头和 header 已发送后失败。

### 状态与 `Allow`

- `kind = "not_found"` 时，Fallback 脚本必须返回 404；`kind = "method_not_allowed"` 时必须返回 405。状态不匹配视为脚本错误，映射为 500 `script_error`。
- 405 的 `Allow` header 由宿主拥有。脚本不得自行设置该 header；宿主在脚本返回合法 405 后按 `ctx.match.allowedMethods` 注入 `Allow`。
- 脚本设置 `Allow` 会抛可捕获的 `script_error`，与文件流拒绝宿主自有 framing header 的 fail-closed 规则一致。
- 404 与 405 的 Body、`Content-Type` 和其他允许的 header 仍由脚本通过 `ctx.respond` 生成。
- Fallback 脚本抛错、超时、内存超限或没有响应时沿用现有 500 `script_error` / `script_no_response` 映射，不能回退到默认 404。

### 日志

- Fallback 的日志 `route` 字段使用 `name`；未命名时使用固定标签 `fallback`。
- `ctx.match.kind` 不写入请求 Body 或响应 Body；日志不新增客户端可控的自由文本。
- 404/405 不读取 Body，因此 `request_body_bytes` 只可使用 `Content-Length` 的观测值或 `null`，不得声称已读取 Body。

### 兼容性

`[fallback]` 与 `ctx.match` 是对配置契约和 `apiVersion` 1 的增量添加，依据 [ADR 0011](0011-contract-compatibility.md) 不提升版本。未使用新配置的现有服务保持原行为；实现落地时同步更新配置、`ctx` 契约与类型定义。

## 后果

**正面**：未知路径与错误方法有稳定、可区分的分类；应用可以返回精确的 JSON 错误体，同时 `Allow` 仍由宿主保证正确。Fallback 继续使用同一脚本执行模型，不需要专用 OAuth 分支或第二套错误响应配置。

**负面（接受）**：配置面增加一个可选表；Fallback 无法读取 Body，因此不能兼作 SPA fallback 或通用兜底 Route。若将来需要通用 fallback，应另行设计，不能悄悄放宽本决策。

**约束（不可破坏）**：普通 Route 的声明顺序与“第一个完全命中获胜”不变；Fallback 只能处理错误响应；不得在引擎中写死应用错误码；不得让 Fallback 通过流式响应绕过状态与 header 校验。

## 验证义务

- 未知路径、已知路径错误方法、正常方法和多个同路径 Route 的 `Allow` 去重顺序。
- 已知路径的错误方法即使带超大或未完成的 Body 也返回 405，不触发 413、multipart 解析或 Body 超时。
- 未配置 Fallback 时未知路径和方法错误都保持 404。
- Fallback 返回错误状态、设置 `Allow`、使用流式 Body、抛错或没有响应时均 fail closed。
- 正常 Route 的声明顺序优先、参数捕获、worker 并发限制与日志字段不回归。

## 替代方案

- **引擎内置 405 和通用错误体**：无法在不增加另一套模板/响应配置的情况下生成应用 JSON，并会把错误格式耦合进引擎，拒绝。
- **使用通配 Route `method = "*"` / `path = "/*"`**：需要重新定义通配匹配、声明顺序、Allow 和 Body 读取语义，容易形成第二套匹配器，拒绝。
- **为每个已知路径声明一个错误方法 Route**：配置重复且无法覆盖任意未知方法，拒绝。
- **Fallback 作为通用响应 Route**：会扩大为 SPA 兜底能力，需要读取 Body、匹配参数并定义与正常 Route 的优先级；超出当前真实场景，拒绝。

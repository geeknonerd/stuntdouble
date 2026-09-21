# `ctx` 宿主 API 契约

[English](./ctx-api.md) \| **中文**

> 本页是英文版 [ctx-api.md](./ctx-api.md) 的译本；如有出入，以英文版为准。

- 状态：draft；切片 T2–T6 实现了 `apiVersion`、`request`、`http.get`、`http.pipe`、`respond`、`log` 与 `env` 子集
- 适用：`apiVersion` 1
- 稳定性：同一 `apiVersion` 内只做增量；移除需要新的 `apiVersion`

## 版本

每个脚本上下文都暴露 `ctx.apiVersion`。首个值是 `"1"`。

同一个 `apiVersion` 内：

- 可以新增宿主函数。
- 不得破坏既有函数名、参数顺序与返回形状。
- 移除或改名函数需要新的 `apiVersion`。
- 产品可以同时支持多个 `apiVersion`。

## 能力分组

| 分组 | API | 状态 |
| --- | --- | --- |
| Request | `ctx.request` | 已实现（T2） |
| 上游 HTTP | `ctx.http.get` | 已实现（T4） |
| 上游 HTTP | `ctx.http.request` | 未实现 |
| 二进制透传 | `ctx.http.pipe` | 已实现（T6） |
| 文件 | `ctx.file.readText` / `ctx.file.readBytes` / `ctx.file.stream` | 未实现 |
| 响应 | `ctx.respond` | 已实现（T2） |
| 请求内状态 | `ctx.local` | 未实现 |
| 日志 | `ctx.log.info` / `warn` / `error` | 已实现（T2） |
| 环境变量 | `ctx.env` | 已实现（T2） |
| 定时器 | `setTimeout` / `setInterval` | 未实现 |

## 已实现子集（切片 T2–T6）

- `ctx.apiVersion` 为 `"1"`。
- `ctx.request` 是只读快照，含 `method`、`path`、`params`、`query`、`headers` 与 `bodyText`。Header 名小写化；请求 body 不是合法 UTF-8 时 `bodyText` 为 `null`。
- `ctx.respond(status, headers, body)` 接受 `[100, 599]` 范围内的状态码、对象或 `[name, value]` 对形式的 headers，以及字符串、字节数组或 `Uint8Array` 类型的 body。第一次调用生效；后续调用被忽略并在服务端产生警告。
- `ctx.http.get(url, opts)` 执行 allowlist 约束的上游 GET，返回 `{status, headers, text(), bytes()}`。
  - `url` 必须是绝对的 `http` 或 `https` URL，其 host 大小写不敏感地匹配 `upstream.allow_hosts`；端口不参与匹配，IP 字面量与 `localhost` 需要显式条目。
  - `opts` 必须是普通对象，且只接受 `{ timeout_ms }`。未知字符串或 symbol 键、继承键、非对象值，以及显式 `null`、`NaN`、`Infinity`、非整数或非正数的 `timeout_ms` 都是脚本错误（fail-closed）。
  - 超时默认取 `upstream.timeout_ms`（15000），`opts.timeout_ms` 按调用覆盖。`opts.timeout_ms` 是上限：有效上游超时受剩余脚本预算约束，并预留一小段回复余量，使超时能表现为上游失败。
  - 重定向由宿主手动跟随，最多 3 跳；每跳前都重新校验协议与 host allowlist。
  - `status` 与 `headers` 是快照；header 名小写化，同名重复取值以最后一个为准。`text()` 以有损方式解码 UTF-8；`bytes()` 返回 `Uint8Array`。每次调用的响应 body 上限为 8 MiB；更大或二进制的载荷属于 `ctx.http.pipe`。
  - 上游请求是直连的。不使用环境代理变量（`HTTP_PROXY`、`HTTPS_PROXY`、`ALL_PROXY` 及小写变体）。
  - 上游 4xx/5xx 响应是数据，绝不抛出。DNS、连接、TLS 与超时失败抛出可捕获错误，其 `error.code` 为 `"upstream_unreachable"`；未捕获时该请求返回 502 `upstream_unreachable` 并带 `request_id`。
- `ctx.http.pipe(url, opts)` 把一次 allowlist 约束的上游 GET body 直接流式转发到客户端响应；字节从不进入脚本堆。
  - `url` 遵循与 `ctx.http.get` 相同的绝对 URL 与 allowlist 规则。重定向由宿主手动跟随，最多 3 跳，每跳都重新校验协议与 host。
  - `opts` 可选，且只接受 `{status, headers}`。`status` 必须是 `[100, 599]` 范围内的整数；`headers` 接受与 `ctx.respond` 相同的对象或 `[name, value]` 对形状。未知键、非普通对象与畸形值都是脚本错误（fail-closed）。
  - `status` 默认取上游 2xx 状态，因此普通下载答 `200`，Range 请求答 `206` 时保留其部分响应状态。脚本提供的 headers 按原样发送；除脚本设置了同名 header，上游的 `Content-Range` 与 `Content-Length` 会被保留。
  - 客户端的 `Range` 请求 header 会转发给上游调用。
  - 上游响应在 `[200, 299]` 范围时开始流式传输。最终非 2xx 响应抛出可捕获错误，其 `error.code` 为 `"upstream_http_error"`；URL 无法解析或 scheme 不是 `http`/`https` 时抛出 `"upstream_url_invalid"`；宿主无法跟随的重定向链（超过 3 跳或 `Location` 不可用）抛出 `"upstream_redirect_error"`；DNS、连接、TLS 与超时失败抛出 `"upstream_unreachable"`；allowlist 拒绝抛出 `"script_error"`。
  - 第一次 `ctx.respond` 或 `ctx.http.pipe` 调用生效；之后的调用被忽略并在服务端产生警告。未捕获的 `upstream_http_error` 是普通脚本错误（500 `script_error`），绝不变成 `502 upstream_unreachable`。
  - 上游 body 读取始终受有效上游超时约束；已经开始流式传输的 body 不能被变换，也不能转成缓冲响应；需要字节的脚本请用 `ctx.http.get`。流一旦开始，body 中途的上游失败只能截断客户端 body，因为状态与 headers 已经在网络上发出。
- `ctx.env` 是进程环境变量快照。不加载 `.env` 文件。
- `ctx.log.info` / `warn` / `error` 只写入服务端日志，绝不进客户端响应。消息不会被脱敏或过滤：脚本作者必须确保消息中不含请求/响应 body、Token、Cookie 或其他机密。宿主自身不会自动记录 body。
- 脚本抛错、超过 `sandbox.script_timeout_ms`（默认 10000）或加载失败时返回 500 `script_error`；脚本结束却没有调用 `ctx.respond` 时返回 500 `script_no_response`；未捕获的上游传输层失败返回 502 `upstream_unreachable`。三者都带 `request_id`。

## 安全边界

脚本拿不到裸 `fetch`、`fs`、`os`、`subprocess` 或 `socket`。所有外部能力都来自宿主函数，并受以下约束：

- 脚本超时
- 内存上限
- 网络 allowlist
- 静态文件根限制
- 上传大小上限
- 堆栈绝不返回给客户端

## 类型定义

产品为每个受支持的 `apiVersion` 发布 `.d.ts` 文件。类型文件是公开契约的一部分。

当前已实现子集的 `apiVersion` 1 源码定义在 [`types/ctx-api-v1.d.ts`](../../types/ctx-api-v1.d.ts)。T8（#11）会把它随发布产物一同发布，并随后续能力分组落地而扩展。

## 弃用

- 移除前至少提前一个 minor 版本警告。
- 被移除的函数需要新的 `apiVersion`。
- 迁移说明写入 `CHANGELOG.md` 与 release notes。

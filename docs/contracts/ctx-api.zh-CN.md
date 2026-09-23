# `ctx` 宿主 API 契约

[English](./ctx-api.md) \| **中文**

> 本页是英文版 [ctx-api.md](./ctx-api.md) 的译本；如有出入，以英文版为准。

- **状态**：v1 切片已公开并冻结；本页描述已实现子集（T1–T12）
- **适用**：`apiVersion` 1
- **稳定性**：同一 `apiVersion` 内只做增量；移除需要新的 `apiVersion`

## 版本

每个脚本上下文都暴露 `ctx.apiVersion`。首个值是 `"1"`。

同一个 `apiVersion` 内：

- 可以新增宿主函数；
- 不得破坏既有函数名、参数顺序与返回形状；
- 移除或改名函数需要新的 `apiVersion`；
- 产品可以同时支持多个 `apiVersion`。

## 功能集

| 分组 | API | 状态 |
| --- | --- | --- |
| 请求快照 | `ctx.request.method` / `path` / `params` / `query` / `headers` / `bodyText` | 已实现（T2） |
| 请求快照 | `ctx.request.files` | 已实现（T12） |
| 请求快照 | `ctx.request.bodyBytes` | pending（不在本切片） |
| 上游 HTTP | `ctx.http.get` | 已实现（T4） |
| 上游 HTTP | `ctx.http.get` 的 `opts.retries` / `backoff` | pending |
| 上游 HTTP | `ctx.http.request` | pending |
| 二进制透传 | `ctx.http.pipe` | 已实现（T6） |
| 文件 | `ctx.file.readText` / `ctx.file.readBytes` / `ctx.file.stream` | 已实现（T11） |
| 响应 | `ctx.respond` | 已实现（T2） |
| 请求内状态 | `ctx.local` | pending |
| 日志 | `ctx.log.info` / `warn` / `error` | 已实现（T2） |
| 环境变量 | `ctx.env` | 已实现（T2） |
| 定时器 | `setTimeout` / `setInterval` | pending |

## 已实现的子集

- `ctx.apiVersion` 为 `"1"`。
- `ctx.request` 是只读快照，含 `method`、`path`、`params`、`query`、`headers`、`bodyText` 与 `files`。Header 名小写化；请求 body 不是合法 UTF-8 时 `bodyText` 为 `null`。已解析的 multipart 请求同样把 `bodyText` 暴露为 `null`，因为 framing 在脚本运行前已被消费。Query 名与值都会做百分号解码，Header 名小写化。重复的 query 或 Header 名在快照中保留最后一个值。
- `ctx.respond(status, headers, body)` 接受 `[100, 599]` 范围内的状态码、对象或 `[name, value]` 对形式的 headers，以及字符串、字节数组、`Uint8Array`、`ArrayBuffer` 或 `ctx.file.stream` / `ctx.request.files[].stream()` handle 类型的 body。第一次调用生效并返回 `true`；后续调用被忽略、返回 `false`，并在服务端产生警告。字节数组的每个值必须是 `[0, 255]` 范围内的整数。Header 名与值在调用时校验；畸形 pair 抛出可捕获的 `script_error`，不会记录 Response，也绝不静默丢弃。
- `ctx.http.get(url, opts)` 执行 allowlist 约束的上游 GET，返回 `{status, headers, text(), bytes()}`。
  - `url` 必须是绝对的 `http` 或 `https` URL，其 host 大小写不敏感地匹配 `upstream.allow_hosts`；端口不参与匹配，IP 字面量与 `localhost` 需要显式条目。
  - `opts` 必须是普通对象，且只接受 `{ timeout_ms }`。未知字符串或 symbol 键、继承键、非对象值，以及显式 `null`、`NaN`、`Infinity`、非整数或非正数的 `timeout_ms` 都是脚本错误（fail-closed）。
  - 超时默认取 `upstream.timeout_ms`（15000），`opts.timeout_ms` 按调用覆盖。`opts.timeout_ms` 是上限：有效上游超时受剩余脚本预算约束，并预留一小段回复余量，使超时能表现为上游失败。
  - 重定向由宿主手动跟随，最多 3 跳；每跳前都重新校验协议与 host allowlist。
  - `status` 与 `headers` 是快照；header 名小写化，同名重复取值以最后一个为准。`text()` 以有损方式解码 UTF-8；`bytes()` 返回 `Uint8Array`。每次调用的响应 body 上限为 8 MiB；更大或二进制的载荷属于 `ctx.http.pipe`。
  - 上游请求是直连的。不使用环境代理变量（`HTTP_PROXY`、`HTTPS_PROXY`、`ALL_PROXY` 及小写变体）。
  - 上游 4xx/5xx 响应是数据，绝不抛出。DNS、连接、TLS 与超时失败抛出可捕获错误，其 `error.code` 为 `"upstream_unreachable"`；非法 URL、不支持的 scheme 或 allowlist 拒绝属于 `"script_error"`。未捕获的传输层失败返回 502 `upstream_unreachable` 并带 `request_id`。
- `ctx.http.pipe(url, opts)` 把一次 allowlist 约束的上游 GET body 直接流式转发到客户端 Response；字节从不进入脚本堆。
  - `url` 遵循与 `ctx.http.get` 相同的绝对 URL 与 allowlist 规则。重定向由宿主手动跟随，最多 3 跳，每跳都重新校验协议与 host。
  - `opts` 可选，且只接受 `{status, headers}`。`status` 必须是 `[100, 599]` 范围内的整数；`headers` 接受与 `ctx.respond` 相同的对象或 `[name, value]` 对形状。未知键、非普通对象与畸形值都是脚本错误（fail-closed）。Header pair 在发起上游调用前校验；畸形 pair 抛出可捕获的 `script_error`，且不发起上游请求。
  - `status` 默认取上游 2xx 状态，因此普通下载答 `200`，Range 请求答 `206` 时保留其部分响应状态。脚本提供的 headers 按原样发送；除脚本设置了同名 header，上游的 `Content-Range` 与 `Content-Length` 会被保留。
  - 客户端的 `Range` 请求 header 会转发给上游调用。
  - 上游响应在 `[200, 299]` 范围时开始流式传输。最终非 2xx 响应抛出可捕获错误，其 `error.code` 为 `"upstream_http_error"`；URL 无法解析或 scheme 不是 `http`/`https` 时抛出 `"upstream_url_invalid"`；宿主无法跟随的重定向链（超过 3 跳或 `Location` 不可用）抛出 `"upstream_redirect_error"`；DNS、连接、TLS 与超时失败抛出 `"upstream_unreachable"`；allowlist 拒绝抛出 `"script_error"`。
  - 第一次 `ctx.respond` 或 `ctx.http.pipe` 调用生效；之后的调用被忽略、返回 `false`，并在服务端产生警告。未捕获的 `upstream_http_error` 是普通脚本错误（500 `script_error`），绝不变成 `502 upstream_unreachable`。
  - 上游 body 读取始终受有效上游超时约束；已经开始流式传输的 body 不能被变换，也不能转成缓冲 Response；需要字节的脚本请用 `ctx.http.get`。流一旦开始，body 中途的上游失败只能截断客户端 body，因为状态与 headers 已经在网络上发出。宿主会在请求日志中把该失败记为 `upstream_stream_error`；日志字段见 [CLI 契约](./cli.zh-CN.md)。
- `ctx.file.readText(path)` 与 `ctx.file.readBytes(path)` 读取配置的静态文件根内的一个文件。路径是相对于 `[files] root` 的文件系统路径，绝不按 URL 或其它平台的路径语法解析；绝对路径与任何 `..` 组件在解析前即被拒绝，符号链接只有在规范化后的目标仍位于规范化根内时才会被跟随。`readText` 以严格 UTF-8 解码；`readBytes` 返回 `Uint8Array`。两者上限均为 8 MiB，并抛出可捕获错误：
  - `file_path_invalid` —— 绝对路径、`..` 组件、空路径，或逃逸出根的符号链接；
  - `file_not_found` —— 解析后的路径上没有文件；
  - `file_too_large` —— 文件超过 8 MiB 缓冲读取上限；
  - `file_encoding_error` —— `readText` 读到的字节不是合法 UTF-8；
  - `file_io_error` —— 路径不是普通文件，或其他 I/O 失败。
  未捕获的文件错误是普通脚本错误（500 `script_error`）；宿主绝不把缺失文件自动映射为 404。
- `ctx.file.stream(path)` 打开根内的一个文件，返回不透明、只能消费一次的 handle，且不带任何可读属性。该 handle 仅可作为 `ctx.respond` 的 `body` 参数使用，打开阶段的失败码与缓冲读取相同。
- `ctx.respond(200, headers, ctx.file.stream(path))` 把文件流式发给客户端，字节不进入脚本堆。脚本状态必须是 `200`；`Accept-Ranges`、`Content-Length` 与 `Content-Range` 归宿主所有，脚本为这些 header 提供值——或在文件流上使用非 200 状态——会抛出可捕获的 `script_error`。
  - 合法的单 range（`bytes=N-M`、`bytes=N-` 或 `bytes=-N`）答 `206`，end 偏移截断到文件末尾，并带正确的 `Content-Length` 与 `Content-Range`。不可用的 Range header——不可满足、畸形、多 range，或空文件上的任意 range——答 `416`，带 `Content-Range: bytes */<size>` 且无 body。存在 `If-Range` 时禁用 Range 处理，答完整 `200`。
  - 绝不推断 `Content-Type`，由脚本设置。HEAD 不会自动映射到 GET Route；只有显式声明的 HEAD Route 才会命中。缓冲的 `ctx.respond` body 保持原有的无 Range 行为。
  - 流一旦开始，body 中途的文件读取失败只能截断客户端 body，因为状态与 headers 已经在网络上发出；宿主会在请求日志中把该失败记为 `file_stream_error`。文件路径绝不进入客户端或日志。日志字段见 [CLI 契约](./cli.zh-CN.md)。
- `ctx.request.files` 是按 multipart 顺序排列的只读上传文件数组。每项含 `field`、`filename`、`contentType`（缺失时为 `null`）、`size`、`text()`、`bytes()` 与 `stream()`。
  - `filename` 只保留客户端提供的 basename：`/` 与 `\` 路径组件都会被剥离。绝不暴露临时文件系统路径。
  - 非 multipart 请求暴露 `[]`；非文件表单字段会被忽略，但其字节仍计入 `files.upload_max_bytes`。
  - `text()` 以严格 UTF-8 解码；`bytes()` 返回 `Uint8Array`。两者上限均为 8 MiB，并在适用时抛出可捕获的 `file_too_large` / `file_encoding_error` / `file_io_error` 错误。`stream()` 不限大小、不透明、只能消费一次，且仅可作为同一请求内 `ctx.respond` 的 `body` 参数。
  - 上传 stream 用作 Response body 时，遵循与 `ctx.file.stream` 相同的宿主 framing 与单 range 200/206/416 规则。
- `ctx.env` 是进程环境变量快照。不加载 `.env` 文件。
- `ctx.log.info` / `warn` / `error` 只写入服务端日志，绝不进客户端 Response。消息不会被脱敏或过滤：脚本作者必须确保消息中不含请求／响应 body、Token、Cookie 或其他机密。宿主自身不会自动记录 body。
- 脚本抛错、超过 `sandbox.script_timeout_ms`（默认 10000）或加载失败时返回 500 `script_error`；脚本结束却没有产生 Response 时返回 500 `script_no_response`；未捕获的上游传输层失败返回 502 `upstream_unreachable`。三者都带 `request_id`。

## Pending 能力

以下能力属于更长的 v1 计划，但尚未实现。它们有意不出现在 `ctx` 与 `types/ctx-api-v1.d.ts` 中；调用不存在的成员是普通脚本错误，映射为 500 `script_error`。

- `ctx.request.bodyBytes`
- `ctx.http.request` 与 `ctx.http.get` 的 retry/backoff 选项
- `ctx.local`
- `setTimeout` 与 `setInterval`

## 安全边界

脚本拿不到裸 `fetch`、`fs`、`os`、`subprocess` 或 `socket`。所有外部能力都来自宿主函数，并受以下约束：

- 脚本应答时限（`sandbox.script_timeout_ms`），到点向客户端返回 500 `script_error`
- 网络 allowlist
- `ctx.file` 的静态文件根限制
- 请求级 multipart 临时存储：`files.upload_max_bytes`、流式计数、8 MiB 缓冲上传读取与 guard 清理
- 堆栈绝不返回给客户端

Boa 0.22 不暴露堆指标、堆上限或 interrupt 钩子，因此没有堆上限被执行；被超时放弃的脚本只能由宿主的循环次数兜底终止。该取舍、其余资源边界与子进程隔离升级路径记录在 [ADR 0003](../../plans/adr/0003-script-first-multi-runtime.md) 的 T3 修订中。

## 类型定义

产品必须为每个受支持的 `apiVersion` 发布 `.d.ts` 文件；[ADR 0012](../../plans/adr/0012-release-artifacts-and-supply-chain.md) 已把它列入发布产物集合。T9 (#12) 接入该流水线。源码定义已经属于本契约。

当前已实现第一切片子集的 `apiVersion` 1 源码定义在 [`types/ctx-api-v1.d.ts`](../../types/ctx-api-v1.d.ts)。在 `apiVersion` 1 内新增能力时，必须在同一次变更中更新该文件与契约。

## 弃用

- 移除前至少提前一个 minor 版本警告。
- 被移除的函数需要新的 `apiVersion`。
- 迁移说明写入 `CHANGELOG.md` 与 release notes。

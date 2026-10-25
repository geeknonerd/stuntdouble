# `ctx` 宿主 API 契约

[English](./ctx-api.md) \| **中文**

> 本页是英文版 [ctx-api.md](./ctx-api.md) 的译本；如有出入，以英文版为准。

- **状态**：v1 切片已公开并冻结；本页描述至 T13 的已实现子集、进程内 Shared state 切片及宿主 Crypto API（ADR 0019）
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
| 请求快照 | `ctx.request.rawQuery` / `bodyBytes` | 已实现（M1.1，v1.x） |
| 请求快照 | `ctx.request.files` | 已实现（T12） |

| 上游 HTTP | `ctx.http.get` | 已实现（T4） |
| 上游 HTTP | `ctx.http.get` 的 `opts.retries` / `backoff` | 已移出 1.0（v1.x 候选，见 ADR 0016） |
| 上游 HTTP | `ctx.http.request` | 已移出 1.0（v1.x 候选，见 ADR 0016） |
| 二进制透传 | `ctx.http.pipe` | 已实现（T6） |
| 文件 | `ctx.file.readText` / `ctx.file.readBytes` / `ctx.file.stream` | 已实现（T11） |
| 响应 | `ctx.respond` | 已实现（T2） |
| 共享状态 | `ctx.state.get` / `set` / `take` / `setIfAbsent` | 已实现（v2 进程内切片） |
| 加密原语 | `ctx.crypto.randomBytes` / `sha256` / `hmacSha256` / `timingSafeEqual` | 已实现（ADR 0019） |
| 请求内状态 | `ctx.local` | 已移出 1.0（v2 候选，见 ADR 0016） |
| 日志 | `ctx.log.info` / `warn` / `error` | 已实现（T2） |
| 环境变量 | `ctx.env` | 已实现（T2） |
| 定时器 | `setTimeout` / `setInterval` | 已移出 1.0（v2 候选，见 ADR 0016） |

## 已实现的子集

- `ctx.apiVersion` 为 `"1"`。
- `ctx.request` 是只读快照，含 `method`、`path`、`params`、`query`、`rawQuery`、`headers`、`bodyText`、`bodyBytes` 与 `files`。Header 名小写化；请求 body 不是合法 UTF-8 时 `bodyText` 为 `null`。已解析的 multipart 请求同样把 `bodyText` 暴露为 `null`，因为 framing 在脚本运行前已被消费。`query` 中的名与值都会做百分号解码。重复的 query 或 Header 名在快照中保留最后一个值。
  - `rawQuery` 是请求行中的原始 query 片段，按客户端发送的原样保留：重复参数名、顺序与百分号编码均不丢失；请求没有 query 时为 `""`。与 `query` 配合可对签名输入做规范化，同时不丢失重复参数。
  - `bodyBytes` 是精确的请求 body 字节，以 `Uint8Array` 提供，不做 UTF-8 转换，因此二进制、NUL 与非法 UTF-8 内容都能逐字节还原。body 未被读取时为 `null`：已解析的 multipart 请求的 framing 在脚本运行前已被消费。空 `Uint8Array` 表示已读取的空 body，与 `null` 可区分。
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
- `ctx.state` 是宿主持有的进程内命名空间，由一个运行中的 `serve` 实例内所有 Route 脚本共享。启动时为空，重启即清空；绝不写入磁盘、环境变量、SQLite、上传目录，也不与其他实例共享。
  - `get(key)` 返回值的独立深拷贝；key 不存在或已过期时返回 `undefined`。读取绝不刷新 TTL。
  - `set(key, value, opts)` 原子插入或替换值，并重置 TTL。没有 `ttl_ms` 时永不过期。
  - `take(key)` 原子返回独立拷贝并删除 key；key 不存在或已过期时返回 `undefined`。
  - `setIfAbsent(key, value, opts)` 是单次原子“检查 + 写入”。写入成功返回 `true`；已有有效值时原值不变并返回 `false`。
  - key 必须是非空 UTF-8 字符串，最多 256 字节，按精确字节比较。value 只能是无损 JSON 数据：`null`、boolean、有限 number、string、稠密数组或普通对象。`undefined`、function、symbol、bigint、非有限 number、`-0`、循环引用、稀疏数组、accessor、隐藏属性或 symbol 属性、非普通对象，以及超过 32 层的嵌套都抛出 `state_invalid_input`。单个 value 的紧凑 JSON 表示最多 16 KiB。该字节数由宿主对解析后的值重新编码得出，不以脚本引擎 `JSON.stringify` 的输出或 number 词法为准。宿主在提交前独立校验 key、TTL、深度、value 与容量；worker 侧检查只用于更早返回可捕获错误。
  - `opts` 必须是普通对象，且只接受 `ttl_ms`。若提供，`ttl_ms` 必须是 `[1, 604800000]`（7 天）范围内的正整数安全整数。TTL 使用宿主单调时钟，从变更提交时开始计算；下一次成功的 `set` 或 `setIfAbsent` 会替换 TTL，读取不续期。
  - 存储最多保留 1024 个有效 key，计费总量最多 8 MiB，按 `UTF-8(key).len() + compact_json(value).len()` 计算。过期项在访问时惰性移除，并在写入可能需要其容量前清理。任一上限耗尽都抛出 `state_capacity_exceeded`；没有 LRU 或静默淘汰，失败的替换保留旧值与旧 TTL。
  - 状态错误可捕获：参数、序列化或 TTL 校验失败抛出 `state_invalid_input`；容量不足抛出 `state_capacity_exceeded`；互斥锁中毒或宿主一致性失败抛出 `state_internal_error`。未捕获的状态错误是普通 500 `script_error`。失败操作不得修改 key 的原值与 TTL。
  - 宿主提交立即生效，且与请求生命周期独立。之后脚本抛错、超时、客户端断开或 worker 崩溃都不会回滚已完成的状态写入。宿主日志绝不记录 state key 或 value。
- `ctx.crypto` 提供最小、可审计的宿主加密原语，全部在 Rust 宿主内实现并经 worker 桥暴露，绝不在 JavaScript 中模拟。
  - 输入接受 `SdCryptoInput`（`string | Uint8Array | ArrayBuffer | readonly number[]`）。字符串按 UTF-8 编码；含孤立 UTF-16 surrogate 会抛出可捕获的 `crypto_invalid_input`，不静默替换字符。数组输入必须为稠密数组且每个元素都是 `[0, 255]` 范围内的整数。非法类型、越界字节值、稀疏数组与超长输入均抛出可捕获的 `crypto_invalid_input`。长度限制一律按编码后的字节数计算，不按 JavaScript 字符数计算。
  - 所有输入在进入宿主时复制；宿主返回新的 `Uint8Array`（或比较操作的 `boolean`），脚本修改返回值不影响宿主。
  - `randomBytes(byteLength)` 从操作系统 CSPRNG 读取指定字节数。`byteLength` 必须是 `[1, 1024]` 内的整数。操作系统熵源失败抛出可捕获的 `crypto_random_unavailable`，不提供降级。
  - `sha256(input)` 返回 32 字节的 `Uint8Array`。输入字节数必须在 `[0, 8 MiB]` 范围内。
  - `hmacSha256(key, message)` 返回 32 字节的 `Uint8Array`。`key` 字节数必须在 `[0, 1 KiB]` 范围内；`message` 字节数必须在 `[0, 8 MiB]` 范围内。
  - `timingSafeEqual(left, right)` 以常数时间比较两个字节序列。每个操作数长度必须在 `[1, 1024]` 字节范围内。两端长度不相等时不按字节短路，直接返回 `false`。
  - 未捕获的 crypto 错误是普通脚本错误，映射为 HTTP 500 `script_error`。宿主绝不将输入、密钥、随机输出、摘要或签名写入请求日志、运维日志或客户端诊断信息。
- `ctx.log.info` / `warn` / `error` 只写入服务端日志，绝不进客户端 Response。消息不会被脱敏或过滤：脚本作者必须确保消息中不含请求／响应 body、Token、Cookie 或其他机密。宿主自身不会自动记录 body。
- 脚本抛错、超过 `sandbox.script_timeout_ms`（默认 10000）、超过内存上限或加载失败时返回 500 `script_error`；脚本结束却没有产生 Response 时返回 500 `script_no_response`；未捕获的上游传输层失败返回 502 `upstream_unreachable`。三者都带 `request_id`。
- 脚本在全新的 worker 进程中运行；宿主拥有的并发槽位池为 4–16 个，数量按可用并行度推导。命中 Route 的脚本拿不到槽位时不会运行，而是快速失败：500 `script_error`；`--verbose` 附加稳定 detail `script worker capacity exhausted`。`sandbox.script_timeout_ms` 到期时，宿主立即杀死 worker 进程并释放其槽位。
- 父进程到 worker 与 worker 到父进程的每条 JSON Lines 协议消息最多 64 MiB。超限属于协议失败：worker 输出超限会终止该 worker，宿主结果超限会作为 bridge 错误被 worker 拒绝。
- Linux 上每个 worker 会在脚本代码运行前把 `sandbox.script_memory_limit_mb`（默认 256 MiB，最低 64）作为 `RLIMIT_AS` 应用到自身。该上限约束虚拟地址空间而非 RSS；因此停止的 worker 返回 500 `script_error`，`--verbose` 附加稳定 detail `script exceeded the configured memory limit`。Windows 上由父进程把每个 worker 放进独立的 Job Object，并以同一预算设置 `JOB_OBJECT_LIMIT_PROCESS_MEMORY`：约束的是提交内存而非地址空间，超限的 worker 返回同样的 500 `script_error` 与 detail；宿主若无法把 worker 放进 job，则拒绝运行而非放任无界执行。本切片中 macOS 无法用 `RLIMIT_AS` 建立有用的上界，因此保留进程隔离与 deadline，但没有硬内存上限。

## 已推迟能力

以下能力经 [ADR 0016](../../plans/adr/0016-v1-ctx-pending-capabilities-deferred.md) 移出 1.0，尚未实现。它们有意不出现在 `ctx` 与 `types/ctx-api-v1.d.ts` 中；调用不存在的成员是普通脚本错误，映射为 500 `script_error`。

- `ctx.http.request` 与 `ctx.http.get` 的 retry/backoff 选项
- `ctx.local`
- `setTimeout` 与 `setInterval`

## 安全边界

脚本拿不到裸 `fetch`、`fs`、`os`、`subprocess` 或 `socket`。所有外部能力都来自宿主函数，并受以下约束：

- 脚本应答时限（`sandbox.script_timeout_ms`），到点向客户端返回 500 `script_error` 并杀死 worker 进程
- 按 worker 设置的内存上限（`sandbox.script_memory_limit_mb`），在脚本代码运行前应用：Linux 用 `RLIMIT_AS` 约束虚拟地址空间，Windows 用 Job Object 约束提交内存；本切片中 macOS 不承诺硬内存上限
- 脚本 worker 进程并发上限（按可用并行度取 4–16 个槽位），槽满时快速失败
- 网络 allowlist
- 宿主持有的进程内状态，具有固定的 key、value、条目、字节与 TTL 边界
- `ctx.file` 的静态文件根限制
- 请求级 multipart 临时存储：`files.upload_max_bytes`、流式计数、8 MiB 缓冲上传读取与 guard 清理
- 堆栈绝不返回给客户端

Boa 0.22 不暴露堆指标、堆上限或 interrupt 钩子。进程隔离 worker 改用操作系统强制的上限：Linux 是 `RLIMIT_AS`，Windows 是父进程持有的 Job Object 提交内存上限；宿主仍通过杀死 worker 进程来终止超时脚本。macOS 在本切片中有进程隔离和 deadline，但没有硬内存上限。相关取舍与其余资源边界记录在 [ADR 0003](../../plans/adr/0003-script-first-multi-runtime.md) 的 T3 修订与 [ADR 0014](../../plans/adr/0014-process-isolated-script-runner.md) 中。

## 类型定义

产品必须为每个受支持的 `apiVersion` 发布 `.d.ts` 文件；[ADR 0012](../../plans/adr/0012-release-artifacts-and-supply-chain.md) 已把它列入发布产物集合。T9 (#12) 接入该流水线。源码定义已经属于本契约。

当前已实现子集（含进程内 Shared state 切片与宿主 Crypto API）的 `apiVersion` 1 源码定义在 [`types/ctx-api-v1.d.ts`](../../types/ctx-api-v1.d.ts)。在 `apiVersion` 1 内新增能力时，必须在同一次变更中更新该文件与契约。

## 弃用

- 移除前至少提前一个 minor 版本警告。
- 被移除的函数需要新的 `apiVersion`。
- 迁移说明写入 `CHANGELOG.md` 与 release notes。

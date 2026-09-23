---
title: "Bridge blocking upstream reads into an async streaming response"
date: 2026-09-21
last_updated: 2026-09-23
category: architecture-patterns
module: upstream HTTP streaming bridge
problem_type: architecture_pattern
component: upstream
severity: medium
applies_when:
  - "Adding a host capability that moves bytes from a blocking upstream reader into an async response body"
  - "Streaming local files (T11) or request uploads (T12) and reusing the pipe pattern"
  - "Changing channel capacity, chunk size, or client-disconnect handling for ctx.http.pipe"
  - "Deciding when response status and headers must be finalized relative to the first body frame"
related_components: [script, server, files]
tags: [streaming, backpressure, axum, tokio, spawn-blocking, ureq, ctx-http-pipe, ctx-file-stream, range, observability]
---

# 把阻塞式上游读取桥接进异步流式响应

## 背景

这条 knowledge-track 学习记录 `ctx.http.pipe` 背后的引擎模式（T6，issue #9，PR #19；T7 的完成日志扩展见 PR #25 与下方「T7 扩展」）。它是架构模式，不是某个路由的错误映射约定：主题是如何把同步、阻塞的字节生产者接到异步 HTTP 响应上，同时不让 body 进入 JavaScript 堆。

脚本宿主是同步的。Boa 的 `Context` 在 `tokio::task::spawn_blocking` 内求值，因此 `ctx.http.get` 与 `ctx.http.pipe` 可以调用 `ureq` 的阻塞客户端而不占用 Tokio 的异步 worker；worker 本身不可取消，会在循环迭代上限处停止（不可取消 worker 与循环上限：`src/script.rs:585-587`、`src/script.rs:765-776`；同步宿主约束另见 `src/upstream.rs:4-8`）。`ctx.http.pipe` 增加了第二个生产者：上游响应头确定之后，body 由阻塞的 `ureq` reader 读取，最终必须送达异步的 axum `Body`。

T11（issue #52）已落地本地文件流：`ctx.file.stream` 复用「由宿主持有有界 channel，而不是脚本堆」这条接缝，错误分类与 framing header 决策则独立定义（见下方「T11 扩展」）。上传方向（T12）仍是该接缝的下一个使用者。

实现分三层：

1. JS prelude 校验调用，只记录 stream 标记与客户端 status/headers（`src/script.rs:379-397`）。
2. 原生桥接调用 `UpstreamAccess::pipe`，把上游 `BodyStream` 保存在请求级 thread-local，只把 status/header 元数据返回给 JavaScript（`src/script.rs:496-525`）。
3. 宿主在求值结束后取走 stream，转换为 `ResponseBody::Stream`，axum 用 `Body::from_stream(ReceiverStream::new(stream))` 适配（`src/script.rs:623-631`、`src/script.rs:665-731`、`src/server.rs:194-201`）。

公开契约给出可观察的结果：body 直接流向客户端，从不进入脚本堆，并保留上游 2xx 状态与相关 range header（`docs/contracts/ctx-api.md:46-53`）。ADR 记录了这条路径为何偏离 `ctx.http.get` 的普通「HTTP 响应即数据」规则（`plans/adr/0005-upstream-failure-semantics.md:45-54`）。

此前会话补了两条约束（会话历史）：宿主函数自 Boa 运行时落地起就是同步的，脚本也从不使用 `async` / `await`，因此即使 body 经由后台 reader 流动，pipe 调用在脚本看来仍然同步；#9 ticket 最初的措辞自相矛盾（「保留上游状态码」与「默认覆盖为 200」），这就是状态规则必须在首帧之前显式定下来、而不能从 `ctx.http.get` 继承的原因。

## 指导

### 1. 让字节流留在 JavaScript 堆之外

`ctx.http.pipe` 不是返回字节的 API。它的 JS wrapper 校验 `{status, headers}`、调用原生桥接，并记录 `{stream: true, status, headers}`；它从不接收 body 字节（`src/script.rs:379-397`）。原生回调把 `PipeBody`（上游 stream 与调用终止句柄）存进 `PIPE_STREAM`，只返回 status/header 的 JSON（`src/script.rs:496-525`）。`ResponseBody::Stream` 在文档注释中明确写着：把帧从上游连接搬到客户端，且不进入 JavaScript 堆（`src/script.rs:108-124`）。

宿主侧表示是带类型的 receiver，而不是 `Vec<u8>` 或 JavaScript 数组：

```rust
pub type BodyStream =
    tokio::sync::mpsc::Receiver<Result<Vec<u8>, std::io::Error>>;
```

`src/upstream.rs` 定义该类型别名；T7 起它是 `PipeBody.stream` 的字段类型，`PipeResponse.body` 是携带 stream 与调用终止句柄的 `PipeBody`。这条边界让脚本决定策略与响应元数据，而响应 body 始终由宿主持有。

### 2. 在暴露 body 之前先定下响应头

`UpstreamAccess::pipe` 有意按这个顺序执行（`src/upstream.rs:166-212`）：

1. 解析并校验 URL，包含首次 allowlist 检查（`src/upstream.rs:167-170`）。
2. 跟随重定向并取得上游响应头（`src/upstream.rs:171-180`）。
3. 在创建 body channel 之前拒绝最终非 2xx 状态（`src/upstream.rs:181-184`）。
4. 合并脚本 header 与上游 range 元数据，然后选定客户端状态（`src/upstream.rs:186-203`）。
5. 创建有界 channel，把阻塞 reader 移入 `spawn_blocking(pump_body)`，返回 `PipeResponse`（`src/upstream.rs:204-211`）。

状态默认值就是上游的 2xx 状态：`client_status.unwrap_or(upstream_status)`（`src/upstream.rs:203`）。因此普通下载答 200，而 Range 响应答 206 时保留 206；测试 `ctx_http_pipe_defaults_to_the_upstream_2xx_status` 用 207 响应证明这是透传而不是硬编码 200（`tests/cli.rs:1423-1435`）。契约记录了同一条规则（`docs/contracts/ctx-api.md:49`）。

这个顺序正是最终非 2xx 无法沿用 `ctx.http.get`「响应即数据」规则的原因。一旦 status 与 headers 返回给异步侧，脚本就无法先检查 body 再改写响应头。因此 T6 修订改为抛出可捕获的 `upstream_http_error`（`plans/adr/0005-upstream-failure-semantics.md:45-54`；`docs/contracts/ctx-api.md:51`）。

### 3. 在流开始前完成失败分类，并保持策略边界

引擎错误变体在 `src/upstream.rs:82-92` 映射为稳定的脚本可见错误码：

| 流开始前的失败 | `error.code` | 当前代码树行为 |
| --- | --- | --- |
| 初始 URL 解析失败，或 scheme 不是 `http`/`https` | `upstream_url_invalid` | `pipe` 把 `Url::parse` 与初始 scheme 校验映射为 `Error::InvalidUrl`（`src/upstream.rs:167-170`、`src/upstream.rs:404-411`）。 |
| 重定向链超过三跳、`Location` 不是可见 ASCII header 值（`HeaderValue::to_str()` 失败）、`Location` 无法 join，或重定向目标使用非 HTTP scheme | `upstream_redirect_error` | `send_following_redirects` 对这些情况调用它的 `redirect_error` 分类器（`src/upstream.rs:240-277`；上限是 `src/upstream.rs:18-19` 的 `MAX_REDIRECTS = 3`）。 |
| DNS、连接、TLS 或超时失败 | `upstream_unreachable` | `Error::Transport` 在 `src/upstream.rs:90-91` 映射；`ctx_http_pipe_transport_failure_is_catchable` 覆盖连接失败（`tests/cli.rs:1303-1322`），pipe 专属的超时目前还没有专门回归测试。 |
| 上游最终非 2xx 响应 | `upstream_http_error` | `pipe` 在创建 channel 之前拒绝该状态（`src/upstream.rs:181-184`）；未捕获时变成普通的 500 `script_error`，脚本也可以捕获并映射（`tests/cli.rs:1266-1301`）。 |
| URL 或 host 被策略拒绝，包括 allowlist 未命中 | `script_error` | 对不在 allowlist 中的 host，`validate` 始终返回 `Error::Policy`，不伪装成上游故障（`src/upstream.rs:401-429`）。 |

allowlist 边界是有意为之。每个重定向目标都会重新校验，allowlist 拒绝始终保持策略错误，即使调用方 API 会把错误 scheme 归类为 `upstream_redirect_error` 或 `upstream_url_invalid`（`src/upstream.rs:240-277`、`src/upstream.rs:401-429`）。demo 路由把 `upstream_url_invalid`、`upstream_http_error`、`upstream_redirect_error`、`upstream_unreachable` 映射为业务 502，但有意重新抛出其他错误，使 allowlist／配置故障保持 `script_error`（`demo/scripts/download.js:75-101`；`tests/cli.rs:1760-1769`）。

当前代码树有一个容易忽略的边界情况：**没有** `Location` 的 3xx 响应不会被 `send_following_redirects` 转成 `Error::Redirect`，而是作为最终响应落回（`src/upstream.rs:256-275`）。接着 `pipe` 把该 3xx 当作非 2xx，抛出 `upstream_http_error` 而不是 `upstream_redirect_error`（`src/upstream.rs:181-184`）。demo 同时捕获这两类并答 `pdf_bad_gateway`，因此它的 502 级测试无法区分二者（`tests/cli.rs:1793-1808`）。按本次会话的结论，不要假定 ADR 中「`Location` 不可用」的措辞涵盖当前实现里缺失 `Location` 的情况；如果这个代码差异有实际影响，需要显式增加宿主分支、对 `error.code` 的回归断言，并同步更新契约与 ADR。

引擎级错误分类只是故事的前半段。客户端可见的业务错误表由路由脚本掌握；这一独立关注点记录在 `docs/solutions/conventions/script-owned-upstream-error-mapping.md`，不应在此重复。

### 4. 用有界 channel 作为同步/异步接缝

pipe 路径不缓冲上游 body。它使用：

- 容量为四帧的 channel（`PIPE_CHANNEL_CAPACITY = 4`，`src/upstream.rs:25-26`）；
- 每次读取最多 64 KiB 的读缓冲（`PIPE_CHUNK_BYTES = 64 * 1024`，`src/upstream.rs:28-29`）；
- `UpstreamAccess::pipe` 中的 `tokio::sync::mpsc::channel(PIPE_CHANNEL_CAPACITY)`（`src/upstream.rs:204-206`）；
- 为阻塞 `ureq` reader 使用 `tokio::task::spawn_blocking`（`src/upstream.rs:205-206`）。

生产者循环 `std::io::Read`，用 `blocking_send` 发送每一帧，把发送失败视为任务结束（`src/upstream.rs:455-480`）。`blocking_send` 就是背压点：reader 无法任意超前于 HTTP 消费者，因此慢客户端不会让宿主缓冲整个文件。同一个发送失败分支也是取消点——当响应 body（以及 receiver）被丢弃时命中（`src/upstream.rs:455-471`）。

这也解释了为什么 `ctx.http.pipe` 不受 `ctx.http.get` 的 body 上限约束。`get` 用 `.limit(MAX_RESPONSE_BYTES).read_to_vec()` 读取（`src/upstream.rs:147-152`），上限是 8 MiB（`src/upstream.rs:21-23`）。pipe 路径从不调用该上限，而是通过有界 channel 流式发送帧。`ctx_http_pipe_streams_bodies_larger_than_the_get_cap` 发送 8 MiB + 1 字节并校验完整长度（`tests/cli.rs:1492-1505`）。按本次会话的结论，正确的内存模型是「有界帧数加上 reader 当前缓冲」，既不是「无界文件」，也不是「与 `get` 相同的 8 MiB 上限」；channel 与读取常量就是预期的边界。

消费者侧在 T6 时同样很小；T7 起由 `stream_body` / `relay_stream` 接管：

```rust
// T6 形态；当前实现见下方「T7 扩展」。
ResponseBody::Stream(stream) => Body::from_stream(ReceiverStream::new(stream)),
```

T7 的 `server::stream_body` 先把 `PipeBody` 解构，经 relay channel 转发到 `Body::from_stream`，并在流结束时写完成日志；`ReceiverStream` 仍负责把 Tokio receiver 变为 `Stream`，axum 仍把它作为响应 body 轮询。

### 5. 保持 range 语义与 header 归属

客户端 `Range` header 在 `evaluate` 创建请求级 `UpstreamAccess` 时从请求快照捕获（`src/script.rs:559-569`）。`ctx.http.get` 明确向 fetch 路径传 `None`，因此不转发客户端 range（`src/upstream.rs:126-134`）。`ctx.http.pipe` 把 `self.client_range` 传入跟随重定向的 fetch 路径（`src/upstream.rs:166-180`），`fetch` 再把它加为上游 `Range` header（`src/upstream.rs:281-295`）。

响应 header 方面，脚本给出的 header 是基础列表。宿主只在脚本没有设置同名（大小写不敏感）header 时复制上游的 `Content-Range` 与 `Content-Length`（`src/upstream.rs:186-202`），不会盲目透传全部上游 header。这样脚本掌握 `Content-Type`、`Content-Disposition` 等 header，同时保留客户端需要的 range 元数据。`ctx_http_pipe_streams_upstream_bytes_with_status_and_headers` 校验脚本 header 与上游 `Content-Length`（`tests/cli.rs:1198-1223`）；demo 的 Range 测试校验 `Range` 抵达上游、客户端状态保持 206、body 为部分内容、`Content-Range` 抵达客户端（`tests/cli.rs:1732-1756`）。

`Content-Length` 是保留而非合成：如果上游使用 chunked 传输且没有提供 `Content-Length`，pipe 路径没有长度可加。demo README 说明此时客户端收到的是 chunked 响应，body 中途失败只能截断它（`demo/README.md:64-70`）。

### 6. 把流开始后的失败当作 body 终止，而不是新状态

`pipe` 返回之后，HTTP 响应头已经固定。如果 `pump_body` 遇到读取错误，它把 `Err(error)` 作为下一个 channel 项发送并停止（`src/upstream.rs:473-477`）。`Body::from_stream` 把该项暴露为 body 错误；它无法追溯修改状态或 header。ADR 与公开契约直接写明后果：流一旦开始，body 中途的上游失败只能截断客户端 body（`plans/adr/0005-upstream-failure-semantics.md:54-56`；`docs/contracts/ctx-api.md:53`）。

这就是错误分类存在硬边界的原因：

- channel 创建**之前**的失败可以变成可捕获的 `error.code`，由脚本映射；
- `pipe` 返回、响应进入流式路径**之后**的失败，只能终止 body 流。

不要为了获得第二次选择状态的机会而缓冲整个响应来「解决」body 中途失败——那会重新制造 pipe 路径本要规避的内存与延迟问题。路由需要检查 body 时，必须改用 `ctx.http.get`，并接受它 8 MiB 的元数据级上限（`docs/contracts/ctx-api.md:38-45`）。

### 7. 让归属保持请求级，清理自动发生

`HTTP_HOST` 与 `PIPE_STREAM` 是 thread-local，不是全局请求状态（`src/script.rs:437-445`）。它们在 `evaluate` 开始时初始化，stream 在求值结束后取走（`src/script.rs:559-574`、`src/script.rs:623-631`）。如果脚本抛错，stream 仍会被取走并由错误路径丢弃；`pump_body` 通过 `blocking_send` 观察到 receiver 消失并退出。预期的生命周期也覆盖客户端断开：当异步响应 body 丢弃 receiver 时，阻塞生产者下一次发送失败，阻塞任务随之结束（`src/upstream.rs:455-471`）。

`script::execute` 的文档说明 `spawn_blocking` 无法取消，外层 deadline 只返回结果，而阻塞 worker 会在循环迭代上限处停止（`src/script.rs:765-771`）。因此流式设计不依赖中止生产者任务，而依赖 receiver 被丢弃。T11 起两个流式路径都有专门的客户端断开回归测试：`ctx_http_pipe_client_disconnect_mid_body_is_logged` 与 `ctx_file_stream_client_disconnect_mid_body_is_logged`；两者都断言完成日志记为 `client_disconnected`，且 relay 未跑完全部字节。

## 为什么重要

1. **它让二进制传输无需脚本堆拷贝。** `ctx.http.get` 返回 `text()`/`bytes()`，上限 8 MiB（`docs/contracts/ctx-api.md:38-45`）；pipe 路径把字节留在有界宿主 channel 中，并测试了超过该上限的情况（`tests/cli.rs:1492-1505`）。没有这条接缝，任何大响应或二进制响应要么失败，要么把显式缓冲策略硬塞进脚本运行时。

2. **它让 HTTP head/body 的顺序显式化。** status 与 headers 只在上游响应头已知之后、body reader 暴露之前选定（`src/upstream.rs:181-211`）。这正是 `upstream_http_error` 这条偏差必要且可预测、而不是与 `ctx.http.get` 偶然不一致的原因（ADR 0005 T6 修订，`plans/adr/0005-upstream-failure-semantics.md:45-54`）。

3. **它让策略失败与上游失败保持可区分。** allowlist 拒绝仍是 `script_error`，而 URL 畸形、不可跟随的重定向、传输层失败与上游最终状态各有可捕获错误码（`src/upstream.rs:82-92`、`src/upstream.rs:401-429`）。因此路由可以答业务 502，同时不掩盖运维配置故障。

4. **它提供背压与清理信号，而不引入第二套执行模型。** 有界 channel 限制预读；T6 时 receiver 丢弃既是客户端断开信号也是生产者退出信号，T7 起改由 `relay_stream` 的 `sender.closed()` 显式观察客户端断开，channel 仍是生产者退出信号（`pump_body` / `relay_stream`）。脚本保持同步，HTTP 层保持异步，唯一的共享对象是由宿主持有的 stream。

## 何时适用

- 新增或评审 `ctx.http.pipe` 行为时：状态选择、重定向处理、allowlist 行为、响应 header 与 Range 语义都穿过同一条「流前／流后」边界（`src/upstream.rs:166-211`；`docs/contracts/ctx-api.md:46-53`）。
- 把另一类字节源流进 axum 响应时。T11 的 `ctx.file.stream` 已按同一「有界 `mpsc` + `Body::from_stream`」接缝落地（读取端是 `tokio::fs::File`，而非 `spawn_blocking` + `ureq`），错误分类与响应头决策针对文件能力单独定义，没有照抄 HTTP 上游语义；上传（T12）是下一个使用者（`docs/contracts/ctx-api.md`）。
- 实现反向上传方向时：沿用同样的有界 channel 与背压原则，但归属与失败映射要贴合上传契约。
- 路由需要在响应前检查、变换或完整校验 body 时：使用 `ctx.http.get` 而不是 `pipe`，并考虑 8 MiB 上限（`docs/contracts/ctx-api.md:43-45`）。
- 变更重定向策略、错误码、Range 转发或 `Content-Length`/`Content-Range` 处理时：在同一次改动里更新 ADR／契约、demo 脚本与端到端测试。上面缺失 `Location` 的细节就是「只断言客户端 502 不够」的具体例证。
- 变更生命周期时：补充或更新对最终非 2xx 可捕获性、重定向上限、非法 URL、超过 8 MiB 的 body、Range/206 保留、客户端断开／背压的检查。

## 示例

### 生产者/消费者交接的典型形态

下面这段 Rust 示意 T6 形态；T7 把 receiver 包进 `PipeBody { stream, call }`（见「T7 扩展」），它不是第二条执行路径：

```rust
// After the upstream head has been accepted and the client status/headers
// have been decided.
let (sender, body) = tokio::sync::mpsc::channel(PIPE_CHANNEL_CAPACITY);
let reader = response.into_body().into_reader();
tokio::task::spawn_blocking(move || pump_body(reader, sender));

Ok(PipeResponse {
    status,
    headers,
    body,
})
```

真实代码在 `src/upstream.rs` 的 `pipe_inner`；T7 起返回 `PipeResponse { status, headers, body: PipeBody { .. } }`。生产者使用 `blocking_send` 而不是 `send`，因为它运行在阻塞 worker 上，必须以同步方式施加背压，不能使用异步上下文中的发送：

```rust
fn pump_body(
    mut reader: impl std::io::Read,
    sender: tokio::sync::mpsc::Sender<Result<Vec<u8>, std::io::Error>>,
) {
    let mut buffer = vec![0_u8; PIPE_CHUNK_BYTES];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                if sender.blocking_send(Ok(buffer[..read].to_vec())).is_err() {
                    break;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => {
                let _ = sender.blocking_send(Err(error));
                break;
            }
        }
    }
}
```

这是 `src/upstream.rs:455-480` 的当前实现；`Interrupted` 分支是有意保留的，因为阻塞读取可能被中断而不代表 body 结束。

### 异步适配器

T6 时 body 直接由这一行适配：

```rust
// T6 形态；T7 起由 server::stream_body/relay_stream 接管，见「T7 扩展」。
ResponseBody::Stream(stream) => Body::from_stream(ReceiverStream::new(stream)),
```

从 T7 开始，`ResponseBody::Stream` 携带 `PipeBody { stream, call }`：`server::stream_body` 先经过一个 relay channel 把字节转发给 axum，并在流结束时写完成日志、定稿上游调用记录。上游 channel 承载 `Vec<u8>`；relay 执行 `Bytes::from(chunk)` 后送入第二个 channel，axum 接收 `Bytes`。帧既不会被合并成单个缓冲，也不会交给 JavaScript。完整设计见下方「T7 扩展」。

### 路由脚本：只捕获该路由负责的错误类别

```js
try {
  ctx.http.pipe(item.pdf_url, {
    headers: {
      "Content-Type": "application/pdf",
      "Content-Disposition": 'attachment;filename="' + String(item.code) + '.pdf"'
    }
  });
} catch (error) {
  if (error.code === "upstream_url_invalid") {
    sendJson(502, "pdf_url_invalid");
    return;
  }
  if (
    error.code !== "upstream_unreachable" &&
    error.code !== "upstream_http_error" &&
    error.code !== "upstream_redirect_error"
  ) {
    throw error;
  }
  sendJson(502, "pdf_bad_gateway");
}
```

这与 `demo/scripts/download.js:75-101` 一致。`throw error` 就是策略边界：allowlist 拒绝不会被转成网关故障。路由级映射约定的完整内容见 `docs/solutions/conventions/script-owned-upstream-error-mapping.md`。

验证接缝本身也是一个持久决策（会话历史）：更早的 spec 会话把端到端检查固定在构建出的二进制 + 真实 HTTP + 标准库 fake upstream 上，因此流式 body 的不变量是通过外部边界断言，而不是对桥接内部做私有断言。

### 当前代码树使用的验证矩阵

| 不变量 | 测试／证据 |
| --- | --- |
| 脚本 status/headers、上游字节与上游 `Content-Length` 随流一起传递 | `ctx_http_pipe_streams_upstream_bytes_with_status_and_headers`（`tests/cli.rs:1198-1223`） |
| 完成日志在 body 结束后写出，并记录精确 relay 字节数 | `ctx_http_pipe_streams_upstream_bytes_with_status_and_headers`（断言完成日志、`response_bytes`、空 error）；通道先关闭时的分类见下方「完成态与客户端断开的竞态（issue #37）」 |
| 中途上游读取失败记为 `upstream_stream_error`，不改变已发出的状态 | `ctx_http_pipe_mid_stream_failure_is_logged_after_headers`（截断 `Content-Length`；断言 error/kind/duration 与 relay bytes；客户端长度只断言 `<=` 上游长度） |
| 客户端中途断开的 `client_disconnected` 分类 | `ctx_http_pipe_client_disconnect_mid_body_is_logged`（8 MiB body，客户端读 1 字节后断开）端到端断言该分类；`StreamOutcome::from_channel_close` 的三个单测覆盖长度匹配、长度不足与无长度三种判定 |
| 最终非 2xx 可作为 `upstream_http_error` 捕获，未捕获时为 500 `script_error` | `tests/cli.rs:1266-1301` |
| 传输层失败可作为 `upstream_unreachable` 捕获 | `tests/cli.rs:1303-1323` |
| 默认状态是上游 2xx 状态，不是硬编码 200 | `ctx_http_pipe_defaults_to_the_upstream_2xx_status`（`tests/cli.rs:1423-1435`） |
| 初始 URL 非法可作为 `upstream_url_invalid` 捕获 | `tests/cli.rs:1438-1459` |
| 超过三次重定向可作为 `upstream_redirect_error` 捕获 | `tests/cli.rs:1462-1489` |
| pipe 可传输超过 `get` 8 MiB 上限的 body | `tests/cli.rs:1492-1505` |
| Range 被转发，206/`Content-Range` 保留 | `demo_download_route_forwards_range_and_preserves_content_range`（`tests/cli.rs:1732-1756`） |
| demo 中 allowlist 拒绝仍是对客户端可见的 `script_error` | `tests/cli.rs:1760-1769` |
| 缺失 `Location` 目前经最终状态路径变成客户端可见的 502 | `tests/cli.rs:1793-1808`；按本次会话的结论，它没有断言 `error.code`，因此无法区分 `upstream_http_error` 与 `upstream_redirect_error` |

## T7 扩展：完成日志与客户端断开（issue #10，PR #25）

T6 在响应头确定后就把 stream 交给 axum，日志也在那时写出；这会把中途截断记录成成功。T7 在桥上再加一段宿主 relay，把「响应已经发出」与「请求已经结束」分开：

- `PipeResponse.body` 改为 `PipeBody { stream, call }`；`call` 是 `StreamCall`，用 `AtomicBool` 保证 `finish` 只定稿一次，并在流结束时写入 `duration_ms`、`response_bytes` 以及失败时的 `upstream_stream_error` / `transport`。
- `server::stream_body` 用第二个有界 channel 把上游 stream 转发给 axum；`relay_stream` 用 `tokio::select!` 同时观察上游帧与 `sender.closed()`，在 body 正常结束、上游读失败或客户端断开时退出。
- relay 的定稿顺序是：先 `call.finish(outcome, bytes)`，再用同一终态覆盖 `response_body_bytes`、`elapsed_ms`、`upstream_calls`，最后写唯一一条完成日志。`http.get` 仍在 body 读完后定稿；`http.pipe` 的调用记录改为流结束后定稿。
- 中途上游读失败记为 `upstream_stream_error`，客户端先离开记为 `client_disconnected`；状态码已经发出，日志不会改写客户端状态。
- 脚本丢弃 pipe 响应（例如 pipe 之后抛错）时，`StreamCall` 的 Drop 以「已放弃」定稿，避免调用链留下半开记录。

### 为什么不能在响应头阶段落日志（session history）

- 在 response-header 阶段，handler 只知道已选定的状态和 headers；body 仍由后台读取线程和 channel 持续生产，最终字节数、正常结束、上游中途读失败以及客户端是否断开都尚未发生。
- 先缓冲完整响应再计算 body size 会抵消 pipe 的流式与内存边界；`ctx.http.get` 的整套缓冲语义不能搬回 pipe。
- 在 header 时写一条、流结束时再补一条会破坏「每请求一条结构化日志」的契约，并造成重复计数与 request_id 相关性混乱；应由同一个幂等的流终态 finalizer 完成。
- header 之后的上游读失败也不能继续映射为 `upstream_unreachable`：header 已送出，无法再把客户端状态改成 502，日志需要独立的 `upstream_stream_error` 终态。

### 完成态与客户端断开的竞态（issue #37）

hyper 在满足 `Content-Length` 后会立即完成响应并 drop body stream，`relay_stream` 的 `sender.closed()` 可能先于上游 EOF 触发；修复前这条路径无条件记 `client_disconnected`，把完整交付误报成客户端断开。现在两条通道关闭路径统一调用 `StreamOutcome::from_channel_close(bytes, announced_content_length)`：已送入字节与声明的 `Content-Length` 匹配时记 `Complete`，否则记 `client_disconnected`；没有声明长度时只可能是客户端提前离开。

E2E harness 在停服前等待请求日志行数稳定（`wait_for_request_log`），避免进程先退出导致完成日志缺失。回归证据：旧实现在 `--test-threads=32` 下 24 轮内第 5 轮复现原始断言失败，修复后同条件 24/24 通过。

### relayed 不等于 delivered

`response_body_bytes` 统计 relay 成功送入响应 body channel 的字节数，不承诺客户端逐字节收到。实测 hyper 在 body stream 报错时可能丢弃已缓冲 chunk，客户端收到的字节可能少于 relay 计数；实现先 `send` 后累加，发送失败的 chunk 不计入，失败路径的回归测试因此断言客户端长度 `<=` 上游长度，同时精确断言 relay 送入的字节数。`http.pipe` 的调用记录同样在流结束时写入 `response_bytes`。

已知边界（v1 不追求连接层精确交付判定）：

- body 在 channel 容量内（最多 4 × 64 KiB）已全部送入、但 hyper 尚未 poll／写出时客户端取消，仍会按 `Complete` 记录；精确区分需要一个能观察连接层写完成的 body adapter，超出当前冻结契约的范围。真实误报出现时再升级。
- HEAD、204、304 这类 hyper 不发送 body 的响应可能被记为 `client_disconnected`；`ctx.http.pipe` 的业务用法默认是 2xx 带 body，暂不为该组合增加分支。

T6 段落中的代码行号会随代码演进漂移；行为以公开契约与本节为准。

## T11 扩展：本地文件流（issue #52）

T11 把同一接缝用于本地文件：`ctx.file.stream(path)` 打开 root 内的文件，`ctx.respond(200, headers, handle)` 把 `FileBody` 交给宿主，`server::relay_file_stream` 经有界 channel 写入 `Body::from_stream`，并在 body 结束、读失败或客户端断开时定稿 `file_calls`。

- 复用：宿主持有有界 channel、`Body::from_stream`、完成时写唯一一条请求日志、客户端断开时丢弃 receiver 让发送端退出。
- 偏离：读取端是 `tokio::fs::File`（异步接口包装阻塞读），不是 `spawn_blocking` + `ureq`；错误分类独立为 `file_stream_error` / `client_disconnected`，不复用 `upstream_stream_error`。
- 响应 framing 由宿主拥有：`Accept-Ranges`、`Content-Length`、`Content-Range` 与 200/206/416 决策都在 `server::map_file_response`；脚本状态必须为 200，Range 解析留在树内且只支持单 range。
- 打开时机独立：`src/files.rs` 在 `File::open` 之前先确认普通文件，FIFO 之类不会阻塞脚本；root 约束与剩余 TOCTOU 取舍见 `SECURITY.md`。

回归证据：`ctx_file_stream_answers_single_ranges_with_206`、`ctx_file_stream_answers_416_for_unusable_ranges`、`ctx_file_stream_client_disconnect_mid_body_is_logged`、`ctx_file_stream_truncation_after_headers_is_logged_as_file_stream_error`。

## 相关

- `plans/adr/0005-upstream-failure-semantics.md` —— T6 修订记录流式偏差与流前／流后的失败划分，T7 修订记录完成日志与日志专用错误类。
- `docs/contracts/cli.md` —— per-request 日志字段、流式完成分类与 `--verbose` 行为。
- `docs/contracts/ctx-api.md` —— `ctx.http.pipe` 的公开契约，含状态默认值、错误码、Range 转发与 body 中途截断。
- `docs/solutions/conventions/script-owned-upstream-error-mapping.md` —— 路由级业务错误映射；本文有意把那张表留给它。
- `tests/cli.rs` —— 针对传输、重定向、URL、状态、大小与 Range 不变量的端到端 fake-upstream 覆盖。
- PR #19 —— T6 模式的实现与验证上下文（已合并）。T7 的完成日志修复见 PR #25（已合并，关闭 issue #10）。
- 相关 issue：#9（T6 来源）、#10（T7 可观测性完成，已关闭）、#7（allowlist 与传输边界）、#8（路由级错误映射）、#37（完成态误报与通道关闭分类）、#3（父 spec）、#52（T11 本地文件流已落地；上传 T12 待做）。

---
title: "Route scripts own upstream error mapping but must not absorb policy rejections"
date: 2026-09-21
category: conventions
module: upstream HTTP error mapping
problem_type: convention
component: upstream
severity: medium
applies_when:
  - "Adding or reviewing a route script that calls `ctx.http.get`"
  - "Adding or reviewing a route script that streams bytes through `ctx.http.pipe`"
  - "Deciding which failures answer the scenario's 502 error code and which stay 500 `script_error`"
  - "Writing end-to-end tests that serve the in-repo demo fixture against a fake upstream"
related_components: [script, upstream, demo]
tags: [error-mapping, upstream, script, demo-fixture, fail-closed, manifest, download, pipe, streaming, range, end-to-end-tests]
---

# 路由脚本负责上游错误映射，但不得吸收策略拒绝

## 背景

T4 确定了 `ctx.http.get` 的引擎边界：最终的非重定向 HTTP 响应（包括 4xx/5xx）是数据，而 DNS、连接、TLS 与超时失败抛出可捕获错误，其 `error.code` 为 `upstream_unreachable`（`docs/contracts/ctx-api.md`、`src/upstream.rs`）。重定向由宿主手动跟随，最多三跳，每跳都重新校验协议与 host（`src/upstream.rs` 中的 `UpstreamAccess::send_following_redirects`）。ADR 0005 说明了划分：有响应意味着语义属于上游，无响应意味着属于 mock，业务调用属于脚本；它的 T6 修订记录了 `ctx.http.pipe` 必须偏离的位置，因为流式 body 从不抵达脚本（`plans/adr/0005-upstream-failure-semantics.md`）。

这条引擎边界不等于客户端可见的错误表。此前会话的一次探查（会话历史，2026-09-20 T4）显示有三个问题被有意留待决定：manifest JSON 解析失败或缺少 `data` 数组属于哪一类、allowlist／URL 策略拒绝应以什么形式暴露、以及随仓库发布的 demo fixture 应如何在测试中驱动。T5（issue #8）在 fixture 内回答了它们并已合并；T6（issue #9）增加了流式下载路由，写作时仍在 PR #19 评审中。两者都在特性分支上从 `demo/` 发布。

`demo/stuntdouble.toml` 声明了 `GET /demo/documents/manifest/:group` 与 `GET /demo/documents/download/:document_id`，`allow_hosts = ["metadata.example.com", "files.example.com"]`。配置契约要求 `files.root` 是已存在的目录（`docs/contracts/config.md`、`src/config.rs`），这就是 fixture 携带 `demo/files/.gitkeep` 的原因。

两个脚本都读取 `ctx.env.METADATA_API_URL`（默认 `https://metadata.example.com/demo/documents`）。manifest 脚本答固定表头 `文件编码,文件标题,系统代码`，行数据按 `code,title,system_code` 顺序；任何包含逗号、双引号、CR 或 LF 的字段都会加引号、内部双引号翻倍，且始终以一个换行结尾；`data` 为空数组时只答表头行（`demo/scripts/manifest.js`）。

## 指导

### 1. 先按来源分类，再决定客户端可见的回答

`ctx.http.get` 保持 ADR 0005 的划分：最终 HTTP 响应（包括 4xx/5xx）是数据；DNS、连接、TLS 与超时失败是可捕获的传输错误；allowlist 拒绝保持策略错误。

`ctx.http.pipe` 无法把 body 交给脚本，因此需要自己的分类。宿主抛出以下可捕获的 `error.code`，路由脚本映射它负责的那些：

| 宿主结果 | `error.code` | 说明 |
| --- | --- | --- |
| `ctx.http.pipe` 收到最终非 2xx 响应 | `upstream_http_error` | 流式 body 无法检查，因此客户端错误码由脚本掌握（ADR 0005 T6 修订） |
| URL 无法解析，或 scheme 不是 http/https | `upstream_url_invalid` | 上游元数据有误，而非传输失败 |
| 宿主无法跟随的重定向链（超过 3 跳或 `Location` 不可用） | `upstream_redirect_error` | 与 allowlist 拒绝区分开 |
| DNS、连接、TLS 或超时失败 | `upstream_unreachable` | 也是 `ctx.http.get` 未捕获时的兜底 |
| URL 或 host 被策略拒绝，例如 allowlist 未命中 | `script_error` | 配置故障，绝不伪装成网关失败 |

### 2. 路由脚本定义该路由的错误表

文档清单 demo 采用的映射：

| 上游结果 | 脚本动作 | 客户端可见结果 |
| --- | --- | --- |
| 状态不在 200–299 | `metadataBadGateway(...)` | 502 `{"error":"metadata_bad_gateway"}` |
| `JSON.parse` 失败 | `metadataBadGateway(...)` | 502 `{"error":"metadata_bad_gateway"}` |
| 顶层 `data` 缺失或不是数组 | `metadataBadGateway(...)` | 502 `{"error":"metadata_bad_gateway"}` |
| `error.code === "upstream_unreachable"` | `metadataBadGateway(...)` | 502 `{"error":"metadata_bad_gateway"}` |
| 其他任何策略或配置错误 | 重新 `throw` | 未捕获时为 500 `script_error` |

前四行是脚本的显式决策（`demo/scripts/manifest.js`）。错误 body 是固定 JSON，诊断原因只进服务端日志——绝不含上游 body 或堆栈（`demo/scripts/manifest.js`、`plans/adr/0005-upstream-failure-semantics.md`）。兜底分类在 `src/script.rs`：未捕获的传输失败是 502 `upstream_unreachable`，脚本异常是 500 `script_error`。

文档下载 demo 采用的映射（T6，`demo/scripts/download.js`）：

| 宿主结果 | 脚本动作 | 客户端可见结果 |
| --- | --- | --- |
| 元数据结果（与 manifest 表一致） | `metadataBadGateway(...)` | 502 `{"error":"metadata_bad_gateway"}` |
| 找不到 `document_id` | `sendJson(404, ...)` | 404 `{"error":"document_not_found"}` |
| `error.code === "upstream_url_invalid"` | `sendJson(502, "pdf_url_invalid")` | 502 `{"error":"pdf_url_invalid"}` |
| `upstream_unreachable` / `upstream_http_error` / `upstream_redirect_error` | `sendJson(502, "pdf_bad_gateway")` | 502 `{"error":"pdf_bad_gateway"}` |
| 其他任何策略或配置错误 | 重新 `throw` | 未捕获时为 500 `script_error` |

`metadata_bad_gateway` 是这个 demo 的业务决策，不是对所有路由的强制要求。不变量是：业务错误码由脚本决定，策略拒绝绝不被伪装成上游故障。

### 3. 只捕获该路由负责的错误码，重新抛出策略拒绝

对 `ctx.http.get`，即只捕获传输失败：

```js
try {
  metadata = ctx.http.get(METADATA_URL);
} catch (error) {
  // A rejected call (allowlist, URL policy) is a configuration error and
  // stays a script error; only transport failures are gateway failures.
  if (error.code !== "upstream_unreachable") {
    throw error;
  }
  metadataBadGateway("metadata upstream unreachable");
  return;
}
```

对 `ctx.http.pipe`，即网关类错误加非法 URL 类，策略拒绝仍然穿透：

```js
try {
  ctx.http.pipe(item.pdf_url, { headers: { "Content-Type": "application/pdf" } });
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

两种写法都随 fixture 一起发布（`demo/scripts/manifest.js`、`demo/scripts/download.js`）。在 catch 块里无条件 `ctx.respond(502, ...)` 会把「host 不在 `allow_hosts`」或 scheme 写错变成「上游挂了」，违背 ADR 0005 的 fail-closed 边界。

### 4. 成功路径属于同一份契约

错误分支不得改变成功路径的形状：manifest 答 `Content-Type: text/plain; charset=utf-8`，无论 `data` 有数据还是空数组都以恰好一个 `\n` 结尾；下载答 `Content-Type: application/pdf` 加 `Content-Disposition` 文件名，并原样流式转发上游字节。错误响应答 `application/json; charset=utf-8`，带稳定的 `error` 码（`demo/scripts/manifest.js`、`demo/scripts/download.js`）。

### 5. 通过外部边界测试随仓库发布的 fixture

仓库只有一条端到端接缝：构建出的二进制 + 真实 HTTP（`tests/cli.rs`）。demo fixture 的做法：

- 复制随仓库发布的 fixture，然后只改写两个标记——`port = 3000` 与 `allow_hosts = ["metadata.example.com", "files.example.com"]`——并先断言标记存在，这样 fixture 一改动就会大声失败，而不是悄悄测了别的东西（`tests/cli.rs` 中的 `demo_fixture`）。
- 把 `METADATA_API_URL` 指向标准库 TCP fake upstream，只断言外部行为：状态、content type、精确 body 字节、错误 JSON。
- fake upstream 每个被接受的连接消费一条预设响应（`tests/cli.rs` 中的 `Upstream`），因此 N 个请求的测试需要 N 条响应——下载场景需要一条元数据响应，外加每个 PDF 请求一条；元数据失败测试先给 404 再给 503。
- 在针对被记录请求做否定断言之前，先建立正向对照：测试先证明捕获到的请求头包含 `host:`，再断言没有客户端 `x-request-id` 泄漏（`demo_download_route_does_not_forward_client_request_id`）。
- Range 覆盖同时断言 pipe 两端：被记录的上游请求带 `range:`，客户端响应答 206 且带上游 `Content-Range`（`demo_download_route_forwards_range_and_preserves_content_range`）。
- 对 `ctx.http.pipe` 的错误码，让脚本自己捕获并回答该码，然后断言稳定的类别（`ctx_http_pipe_redirect_limit_is_catchable`、`ctx_http_pipe_url_rejection_is_catchable`）。

## 为什么重要

1. 它让 ADR 0005 的边界保持诚实。「有响应／无响应」是引擎的划分；业务错误表属于脚本，一个 catch-all 会把第二层抹掉。T6 修订记录了一处偏差：`ctx.http.pipe` 对最终非 2xx 响应抛 `upstream_http_error`，因为流式 body 无法成为脚本可检查的数据。
2. 它把依赖故障与运维误配置分开。allowlist 拒绝意味着配置需要改，而不是上游挂了；把它报成 502 会把告警、重试与 on-call 判断引向错误方向，并掩盖一个 fail-closed 控制。畸形的 `pdf_url` 则属于上游数据：路由负责它，回答 `pdf_url_invalid`。
3. 它给调用方稳定的、可断言的表面：`metadata_bad_gateway`、`pdf_url_invalid`、`pdf_bad_gateway` 与 `script_error` 是公开类别。引擎自己的错误 body 只带 `request_id` 与 `error`（`src/server.rs`），且 ADR 0005 要求堆栈、上游 body 与内部地址绝不抵达客户端——脚本若回显它取到的内容，正是会破坏该保证的做法（`plans/adr/0005-upstream-failure-semantics.md`、`docs/contracts/ctx-api.md`）。
4. 它让测试不给出虚假信心：标记断言钉住被测配置，正向对照让否定 header 检查有意义，每个连接一条响应让 N 请求场景始终走在真实网络路径上。

## 何时适用

- 任何调用 `ctx.http.get` 并把上游结果转成自己的客户端可见错误的路由脚本。
- 任何通过 `ctx.http.pipe` 传输字节、且必须把 `upstream_url_invalid`、`upstream_redirect_error`、`upstream_http_error`、`upstream_unreachable` 与 allowlist `script_error` 区分开的路由脚本。
- 任何定义稳定内部错误码、且必须区分上游故障、上游数据错误与本服务自身配置／脚本错误的路由。
- 更新错误表、README、契约描述或 runbook 时：一句「元数据失败答 502」也必须点名 allowlist 拒绝与非法 URL 类，或链接到它们的说明位置。
- 为随仓库发布的配置与脚本 fixture 增加端到端覆盖时，尤其是复制配置、伪造上游或对请求 header 做断言的时候。
- 有意透传上游状态码时：把那当作显式的业务选择，绝不让它掩盖策略错误。

## 示例

### 反模式：catch-all 把配置错误变成 502

```js
try {
  const metadata = ctx.http.get(METADATA_URL);
  // ... build CSV ...
} catch (error) {
  ctx.respond(502, { "Content-Type": "application/json" }, '{"error":"metadata_bad_gateway"}');
}
```

当 host 不在 `allow_hosts`，或 `METADATA_API_URL` 的 scheme 写错时，这段代码仍然回答「上游网关故障」。调用方看到的是可重试的外部依赖问题，而运维永远看不到那个必须修复的配置故障。

### 正确写法及其测试

```js
try {
  metadata = ctx.http.get(METADATA_URL);
} catch (error) {
  if (error.code !== "upstream_unreachable") {
    throw error;
  }
  metadataBadGateway("metadata upstream unreachable");
  return;
}
```

```rust
let upstream = Upstream::start(vec![
    UpstreamResponse::new(404, b"missing"),
    UpstreamResponse::new(503, b"unavailable"),
]);
assert_metadata_bad_gateway(&manifest_response(&upstream, &[]));
assert_metadata_bad_gateway(&manifest_response(&upstream, &[]));

let head = upstream.requests()[0].to_ascii_lowercase();
assert!(head.contains("host:"), "recorded head: {head}");
assert!(!head.contains("x-request-id"), "client header leaked upstream");
```

两次 manifest 请求需要两条预设响应，而 `host:` 是正向对照，让随后针对 `x-request-id` 的否定断言有意义（`demo_manifest_route_maps_metadata_non_2xx_to_502`、`demo_manifest_route_does_not_forward_client_request_id`）。

## 相关

- [ADR 0005 —— 上游失败语义](../../../plans/adr/0005-upstream-failure-semantics.md)
- [`ctx` API 契约](../../contracts/ctx-api.md)
- [演示夹具 README](../../../demo/README.md)
- [T6 issue #9](https://github.com/geeknonerd/stuntdouble/issues/9)、[T5 issue #8](https://github.com/geeknonerd/stuntdouble/issues/8)、[T4 issue #7](https://github.com/geeknonerd/stuntdouble/issues/7)

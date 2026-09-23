# 快速开始

[English](./getting-started.md) \| **中文**

> 本页是英文版 [getting-started.md](./getting-started.md) 的译本；如有出入，以英文版为准。

本文走通最小可用的 Stunt Double 配置：安装二进制、描述一条 Route、启动服务并核对结果。契约细节在[公开契约](../contracts/)里，本文只给链接，不重复内容。

## 前置条件

- Rust 工具链。crate 在 stable Rust 上构建；`Cargo.toml` 声明 MSRV 下限。

## 安装

```bash
cargo install --path .          # 安装 stuntdouble 二进制
# 或直接在检出目录运行
cargo run -- serve --config stuntdouble.toml
```

发布构建还覆盖 Linux x86_64、macOS arm64、Windows x86_64，并发布容器镜像。发布契约要求每个 Release 包含归档、校验和、attestation、SBOM、`ctx` API 类型定义以及 GHCR tag 与 digest；下载与验证命令见 [README 的安装与验证段](../../README.zh-CN.md#安装与验证)。

## 写配置

在脚本旁边创建 `stuntdouble.toml`：

```toml
config_version = "1"

[server]
bind = "127.0.0.1"
port = 3000

[files]
root = "./files"

[[routes]]
name = "hello"
method = "GET"
path = "/hello/:name"
script = "scripts/hello.js"
```

`files.root` 必须存在；它是 `ctx.file` 唯一可读取的目录。[配置契约](../contracts/config.zh-CN.md)列出了全部键与校验规则。

## 写第一个 Route

`scripts/hello.js`：

```js
const name = ctx.request.params.name;
ctx.respond(200, { "Content-Type": "text/plain; charset=utf-8" }, "hello " + name + "\n");
```

## 校验并运行

```bash
stuntdouble validate --config stuntdouble.toml   # 校验 schema 与文件，不打开 socket
stuntdouble serve --config stuntdouble.toml
curl -i http://127.0.0.1:3000/hello/world
```

## 停止服务

按 Ctrl-C（SIGINT），或在 Unix 上发送 SIGTERM。第一个信号会停止接受新连接、排空在途请求，然后以 `0` 退出。

如果关闭耗时过长，可以再发一次信号：graceful shutdown 启动后观测到的第二个信号会放弃排空，立即以 `130`（SIGINT/Ctrl-C）或 `143`（SIGTERM）退出。标准信号不排队，因此第一个信号被观测前背靠背发送的两个信号可能合并；这种情况仍会正常排空并以 `0` 退出。完整规则见 [CLI 契约](../contracts/cli.zh-CN.md)。

## 调用上游 API

上游调用只能到达 `[upstream] allow_hosts` 列出的 host：

```toml
[upstream]
allow_hosts = ["metadata.example.com"]
timeout_ms = 15000
```

```js
const upstream = ctx.http.get("https://metadata.example.com/documents");
if (upstream.status >= 400) {
  ctx.respond(502, { "Content-Type": "application/json" }, '{"error":"metadata_bad_gateway"}');
} else {
  ctx.respond(200, { "Content-Type": "application/json" }, upstream.text());
}
```

4xx/5xx 响应是数据；只有传输层失败才抛 `upstream_unreachable`。body 很大或是二进制时改用 `ctx.http.pipe`，它把上游 body 直接流给客户端并保留 `Range`/206——见[演示夹具](../../demo/README.zh-CN.md)与 [`ctx` API 契约](../contracts/ctx-api.zh-CN.md)。

## 读取与流式发送本地文件

`ctx.file` 只读取 `files.root` 内的文件。绝对路径与任何 `..` 组件都会被拒绝，符号链接必须解析到根内，缓冲读取上限为 8 MiB：

```js
const text = ctx.file.readText("metadata.json");
ctx.respond(200, { "Content-Type": "application/json" }, text);
```

`readText` 以严格 UTF-8 解码；`readBytes` 返回 `Uint8Array`。两者都抛出可捕获错误（`file_path_invalid`、`file_not_found`、`file_too_large`、`file_encoding_error`、`file_io_error`），未捕获时返回 500 `script_error`。

要把文件流给客户端而不进入脚本堆，把 `ctx.file.stream(path)` 作为 `ctx.respond` 的 body 传入（状态必须是 `200`）。`Accept-Ranges`、`Content-Length` 与 `Content-Range` 归宿主所有：合法的单 `Range` 答 206，不可用的 Range 答 416，`If-Range` 存在时禁用 Range 处理。`Content-Type` 不会自动推断，需要脚本在响应 header 中设置。

```js
ctx.respond(200, { "Content-Type": "application/pdf" }, ctx.file.stream("documents/DOC-0001.pdf"));
```

## 处理 multipart 上传

命中的 `multipart/form-data` 请求会在路由脚本运行前由宿主解析。`[files] upload_max_bytes` 限制文件与非文件字段的数据总量（默认 `20971520`），整个 multipart body（含 framing）另有 1 MiB framing allowance。part 畸形或缺少 `name` 属性时返回 400 `invalid_multipart`；数据超限返回 413 `upload_too_large`。两者都使用带 `request_id` 的项目 JSON 错误 envelope，且脚本不会运行。

```js
const file = ctx.request.files[0];
if (!file) {
  ctx.respond(400, { "Content-Type": "application/json" }, '{"error":"document_required"}');
} else {
  ctx.respond(200, { "Content-Type": "text/plain; charset=utf-8" }, file.text());
}
```

`ctx.request.files` 是只读数组，元素形如 `{field, filename, contentType, size, text(), bytes(), stream()}`。`filename` 只保留客户端 basename：`/` 与 `\` 路径组件会被剥离，临时路径绝不暴露。非 multipart 请求暴露 `[]`；非文件字段会被忽略，但仍计入上限。

`text()` 严格按 UTF-8 解码，`text()` 与 `bytes()` 上限为 8 MiB。`stream()` 不限大小、不透明、只能消费一次，且仅可作为同一请求内 `ctx.respond` 的 body；它遵循与 `ctx.file.stream` 相同的 Range 与宿主 framing 规则。上传存放在请求级临时存储中，请求结束或流结束时清理——精确形状见 [`ctx` API 契约](../contracts/ctx-api.zh-CN.md) 与[配置契约](../contracts/config.zh-CN.md)。

## 出错时怎么判断

| 你看到的 | 含义 |
| --- | --- |
| 404 `not_found` | 没有 Route 匹配该 method 与 path |
| 500 `script_error` | 脚本抛错、超时或加载失败；堆栈只留在服务端日志 |
| 500 `script_no_response` | 脚本结束却没有调用 `ctx.respond` |
| 502 `upstream_unreachable` | 未捕获的传输层失败（DNS、连接、TLS 或超时） |
| 带点号路径的校验错误 | 配置违反契约；消息会给出字段名与期望形状 |

`sandbox.script_timeout_ms` 限制脚本运行时长，默认 10000 ms。脚本还受循环次数兜底与递归/VM 栈上限约束，引擎 panic 返回 500 `script_error`。Boa 0.22 不暴露堆指标或 interrupt 钩子，进程内没有堆上限——威胁模型见 [SECURITY.md](../../SECURITY.md)，取舍见 [ADR 0003](../../plans/adr/0003-script-first-multi-runtime.md) 的 T3 修订。

诊断这些失败时，可以给 `serve` 加上 `--verbose`：

```bash
stuntdouble serve --config stuntdouble.toml --verbose
```

此时 500/502 JSON body 会多出一个稳定的 `detail` 字符串，例如 `upstream transport failure: timeout`；它绝不包含堆栈、脚本消息、上游 body 或内部地址。把 `detail` 当作本地诊断输出，不要在共享环境开启 `--verbose`。`serve` 还会为每个请求向 stderr 写出一条结构化日志，包含上游调用链、body 大小与白名单 header。流式响应的日志在 body 结束时写出；中途失败记为 `upstream_stream_error` 或 `file_stream_error`，客户端中途离开记为 `client_disconnected`。

## 下一步

- [公开契约](../contracts/README.zh-CN.md) —— 配置、`ctx` API 与 CLI。
- [演示夹具](../../demo/README.zh-CN.md) —— 清单 CSV 生成、带 Range 的 PDF 流式下载，以及离线文件切片（本地清单、本地下载与 multipart 上传）。
- [产品功能定义](../../plans/product-definition.md) —— v1 范围与不做清单。

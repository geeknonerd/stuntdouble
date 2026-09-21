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

即使暂时没有 Route 读取文件，`files.root` 也必须存在。[配置契约](../contracts/config.zh-CN.md)列出了全部键与校验规则。

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

## 出错时怎么判断

| 你看到的 | 含义 |
| --- | --- |
| 404 `not_found` | 没有 Route 匹配该 method 与 path |
| 500 `script_error` | 脚本抛错、超时或加载失败；堆栈只留在服务端日志 |
| 500 `script_no_response` | 脚本结束却没有调用 `ctx.respond` |
| 502 `upstream_unreachable` | 未捕获的传输层失败（DNS、连接、TLS 或超时） |
| 带点号路径的校验错误 | 配置违反契约；消息会给出字段名与期望形状 |

`sandbox.script_timeout_ms` 限制脚本运行时长，默认 10000 ms。

诊断这些失败时，可以给 `serve` 加上 `--verbose`：

```bash
stuntdouble serve --config stuntdouble.toml --verbose
```

此时 500/502 JSON body 会多出一个稳定的 `detail` 字符串，例如 `upstream transport failure: timeout`；它绝不包含堆栈、脚本消息、上游 body 或内部地址。把 `detail` 当作本地诊断输出，不要在共享环境开启 `--verbose`。`serve` 还会为每个请求向 stderr 写出一条结构化日志，包含上游调用链、body 大小与白名单 header。

## 下一步

- [公开契约](../contracts/README.zh-CN.md) —— 配置、`ctx` API 与 CLI。
- [演示夹具](../../demo/README.zh-CN.md) —— 清单 CSV 生成与带 Range 的 PDF 流式下载。
- [产品功能定义](../../plans/product-definition.md) —— v1 范围与不做清单。

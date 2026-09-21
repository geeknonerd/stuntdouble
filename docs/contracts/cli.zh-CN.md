# CLI 契约

[English](./cli.md) \| **中文**

> 本页是英文版 [cli.md](./cli.md) 的译本；如有出入，以英文版为准。

- **状态**：v0.x 切片 T7 稳定
- **适用**：v0.1.0-alpha.1 及以后、同一配置族内
- **稳定性**：1.0 之前允许带弃用窗口的破坏性变更

## 命令

| 命令 | 用途 | 状态 |
| --- | --- | --- |
| `stuntdouble serve` | 启动 mock server，监听 HTTP 请求 | 已实现 |
| `stuntdouble validate` | 只校验配置文件，不启动服务 | 已实现 |

### serve

用法：

```bash
stuntdouble serve --config <path> [--verbose]
```

启动 mock server 并绑定到配置的地址。命中的 Route 在内置 Boa 运行时中执行其 JavaScript，宿主注入 `ctx`，包含 allowlist 约束的 `ctx.http.get` 调用与流式 `ctx.http.pipe` 调用；未命中的 Route 返回 404 `not_found`。脚本失败返回 500 `script_error` 或 `script_no_response`；未捕获的上游传输层失败返回 502 `upstream_unreachable`。

`--verbose` 会为引擎生成的 500/502 JSON 错误 body 附加 `detail` 字段。它的值是稳定的失败类别，例如 `script execution failed`、`script exceeded the configured timeout`、`upstream transport failure: timeout`、`upstream transport failure: dns` 或 `upstream transport failure: transport`；绝不包含堆栈、脚本消息、上游 body、hostname、IP 地址或 URL。不开启该 flag 时，错误 body 只含 `error` 与 `request_id`。该 flag 只用于本地诊断，不要在共享环境开启。

默认配置路径是 `stuntdouble.toml`。配置必须包含 `[[routes]]`；每条 Route 指定 method、path、脚本位置与可选 name。

#### 退出码

- `0`：成功启动（服务运行直到收到关闭信号）
- `2`：配置错误（TOML 非法或 schema 违规）——消息打印到 stderr
- `3`：内部／服务启动失败

#### 绑定语义

`server.bind` 字段必须是 IP 字面量（`127.0.0.1` 等）。hostname 由操作系统解析，本切片不直接支持。省略 `server.port` 时默认 3000。

#### 请求日志

`serve` 每个请求向 stderr 写出一条 JSON 日志，包含 `request_id`、命中的 `route`、`method`、`path`、`params`、`status`、`error`、`elapsed_ms`、`script_duration_ms`、`upstream_calls`、`request_body_bytes`、`response_body_bytes`、`request_headers`、`response_headers`、`client_request_id`、`host`，以及脚本有日志时的 `script_logs`。

- `upstream_calls` 是脚本发起调用的有序列表。每项记录 `api`（`http.get` 或 `http.pipe`）、`host`、`path`、`status`、`duration_ms`、`redirects`，失败时还记录稳定的 `error`/`kind`。query string 不写入日志。
- 请求体与响应体永不写日志，只记录大小与白名单 header：请求 header 为 `accept`、`content-type`、`content-length`、`range`、`user-agent`；响应 header 为 `content-type`、`content-length`、`content-range`。Authorization、Cookie 及其他 header 永不写日志。
- 没有已知 `Content-Length` 的流式响应，其 `response_body_bytes` 为 `null`。
- 脚本到达截止时间时仍在途的调用，其 `status` 与 `duration_ms` 保持 `null`；截止前完成的调用保留已记录的值。
- `client_request_id` 记录客户端传入的 `X-Request-ID`；它既不会被采用为 `request_id`，也不会转发给上游调用。

### validate

用法：

```bash
stuntdouble validate --config <path>
```

加载配置文件、校验必填字段与类型，把诊断细节打印到 stderr，把 "OK" 打印到 stdout。始终在打开 socket 之前退出。配置非法时，命令会打印违规项，包含点号字段名、期望形状与实际值。

配置失败时退出码为 `2`，否则为 `0`。

## 全局 flag

`-c, --config` 是全局 flag，适用于所有命令：

- `stuntdouble --config x serve` —— 等价于 `stuntdouble serve --config x`
- `stuntdouble --config x validate` —— 等价于 `stuntdouble validate --config x`

| Flag | 用途 | 状态 |
| --- | --- | --- |
| `-c, --config <path>` | 选择配置文件（默认 `stuntdouble.toml`） | 已实现 |
| `-h, --help` | 打印帮助 | 已实现 |
| `-V, --version` | 打印版本元数据（`stuntdouble <version>`） | 已实现 |

## 退出码

除 `serve` 运行期退出外，所有命令都使用这些退出码。

| 码 | 含义 |
| ---: | --- |
| `0` | 成功 |
| `1` | 运行时错误（仅 `serve` 运行期间报告） |
| `2` | 配置错误 |
| `3` | 内部错误 |

## 弃用

- 移除或改名 flag 或命令前，至少提前一个 minor 版本警告。
- 警告写入 stderr 与 `CHANGELOG.md`。
- `1.0` 及以后按 SemVer 保证 CLI 兼容性。

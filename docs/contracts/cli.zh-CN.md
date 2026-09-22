# CLI 契约

[English](./cli.md) \| **中文**

> 本页是英文版 [cli.md](./cli.md) 的译本；如有出入，以英文版为准。

- **状态**：v1 切片已冻结；增量更新至 T11
- **适用**：v0.1.0-alpha.1 及以后、同一 CLI 家族内的 `stuntdouble` 二进制
- **稳定性**：1.0 之前的破坏性变更需带弃用窗口

## 命令

| 命令 | 用途 | 状态 |
| --- | --- | --- |
| `stuntdouble serve` | 启动 mock server，监听 HTTP 请求 | 已实现 |
| `stuntdouble validate` | 只校验配置文件，不启动服务 | 已实现 |

### serve

用法：

```bash
stuntdouble serve [--config <path>] [--verbose]
```

启动 mock server 并绑定到配置的地址。命中的 Route 在内置 Boa 运行时中执行其 JavaScript，宿主注入 `ctx`，包含 allowlist 约束的 `ctx.http.get` 调用、流式 `ctx.http.pipe` 调用，以及带根限制的 `ctx.file` 读取与流式本地文件响应；未命中的 Route 返回 404 `not_found`。脚本失败返回 500 `script_error` 或 `script_no_response`；未捕获的上游传输层失败返回 502 `upstream_unreachable`。

`--verbose` 会为引擎生成的 500/502 JSON 错误 body 附加 `detail` 字段。它的值是稳定的失败类别，例如 `script execution failed`、`script exceeded the configured timeout`、`upstream transport failure: timeout`、`upstream transport failure: dns` 或 `upstream transport failure: transport`；绝不包含堆栈、脚本消息、上游 body、hostname、IP 地址或 URL。不开启该 flag 时，错误 body 只含 `error` 与 `request_id`。该 flag 只用于本地诊断，不要在共享环境开启。404 `not_found` 响应永远不带 `detail`。

默认配置路径是 `stuntdouble.toml`。配置必须包含非空 `routes` 数组；每条 Route 指定 method、path、脚本位置与可选 name。完整 schema 规则见[配置契约](config.zh-CN.md)。

#### serve 退出码

| 码 | 含义 |
| ---: | --- |
| `0` | server 正常返回（包括完成 graceful shutdown）；`validate`、`--help` 与 `--version` 也以 `0` 表示成功 |
| `1` | 运行期／服务错误，包括 socket bind 或 listen 失败 |
| `2` | 配置错误（TOML 非法、schema 违规、未知 `config_version` 或非 IP 的 `server.bind`）或 CLI 用法错误（未知 flag、缺少子命令）——消息打印到 stderr |
| `3` | 构造异步运行时的内部错误 |
| `130` | graceful shutdown 进行中再次收到 Ctrl-C/SIGINT，放弃排空 |
| `143` | graceful shutdown 进行中再次收到 SIGTERM，放弃排空 |

#### 关闭信号

在 Unix 上，`serve` 处理 SIGINT（Ctrl-C）与 SIGTERM；在 Windows 上处理 Ctrl-C（等同 SIGINT）。第一个信号会向 stderr 写一行诊断，让服务器停止接受新连接，并排空在途请求后以退出码 `0` 返回。

如果第二个信号在第一个信号已经启动 graceful shutdown 后到达，它会放弃排空并立即终止进程：SIGINT/Ctrl-C 退出码为 `130`，SIGTERM 为 `143`。仅在必须立即放弃在途请求时使用。

标准信号不排队。若两个信号在第一个信号被观测前背靠背到达，它们可能被合并为一个通知；这种情况只遵循第一个信号，正常排空并退出 `0`。关闭诊断的具体文案不属于本契约。

#### 绑定语义

`server` 表是必填项。`bind` 字段可选，默认 `127.0.0.1`；`port` 可选，默认 `3000`。

`server.bind` 必须是 IP 地址字面量（`127.0.0.1`、`::1` 等）。hostname 在配置校验期以退出码 `2` 被拒绝；本切片 CLI 不解析 hostname。

#### 请求日志

`serve` 每个请求向 stderr 写出一条 JSON 日志，包含 `request_id`、命中的 `route`、`method`、`path`、`params`、`status`、`error`、`elapsed_ms`、`script_duration_ms`、`upstream_calls`、`file_calls`、`request_body_bytes`、`response_body_bytes`、`request_headers`、`response_headers`、`client_request_id`、`host`，以及脚本有日志时的 `script_logs`。

- `upstream_calls` 是脚本发起调用的有序列表。每项记录 `api`（`http.get` 或 `http.pipe`）、`host`、`path`、`status`、`response_bytes`、`duration_ms`、`redirects`，失败时还记录稳定的 `error`/`kind`。query string 不写入日志。`http.pipe` 的记录在 body 流结束时定稿。
- `file_calls` 是脚本发起的文件调用有序列表，记录 `api`（`file.readText`、`file.readBytes` 或 `file.stream`）、`bytes`、`duration_ms`，失败时还记录稳定的 `error`。文件路径绝不写入日志。`file.stream` 的记录在 body 结束或客户端断开时定稿。
- 宿主不会自动记录请求体或响应体，只记录大小与白名单 header：请求 header 为 `accept`、`content-type`、`content-length`、`range`、`user-agent`；响应 header 为 `content-type`、`content-length`、`content-range`。Authorization、Cookie 及其他 header 永不写日志。`script_logs[].message` 由脚本产生且不做脱敏：`ctx.log.*` 中不得包含 body、Token、Cookie 或其他机密。
- 缓冲 Response 在写出前落日志。流式 Response（`ctx.http.pipe` 或 `ctx.file.stream`）在 body 结束或客户端断开后落一条完成日志：`response_body_bytes` 统计转发进 Response body 的字节数；`error` 可能是 `upstream_stream_error` 或 `file_stream_error`（状态已经发出后读取失败，客户端状态保持已发送值）或 `client_disconnected`。该行的 `elapsed_ms` 覆盖整个流。
- 脚本到达截止时间时仍在途的 `http.get` 调用，其 `status`、`response_bytes` 与 `duration_ms` 保持 `null`；截止前完成的调用保留已记录的值。
- 这些日志是运维诊断面，可能包含 allowlist 中的上游 host 与 path；发布到 issue、pull request 或其他公开产物前必须脱敏。
- `client_request_id` 记录客户端传入的 `X-Request-ID`；它既不会被采用为 `request_id`，也不会转发给上游调用。

### validate

用法：

```bash
stuntdouble validate [--config <path>]
```

加载配置文件，校验必填字段、类型、默认值与[配置契约](config.zh-CN.md)中的文件系统检查。它始终在打开 socket 之前退出，也从不读取 Route 脚本。

成功时，命令向 stdout 打印 `<path>: valid configuration`，并向 stderr 打印一行简短摘要（`config_version`、server 地址、`files.root` 与 Route 数量）。失败时，它向 stderr 打印文件路径与全部违规项，包含点号字段名、期望形状与实际值。

配置非法、无法读取或 CLI 参数非法时退出码为 `2`，否则为 `0`。

## 全局 flag

`-c, --config` 是全局 flag，适用于所有命令：

- `stuntdouble --config x serve` 等价于 `stuntdouble serve --config x`
- `stuntdouble --config x validate` 等价于 `stuntdouble validate --config x`

| Flag | 用途 | 状态 |
| --- | --- | --- |
| `-c, --config <path>` | 选择配置文件（默认 `stuntdouble.toml`） | 已实现 |
| `-h, --help` | 打印帮助 | 已实现 |
| `-V, --version` | 打印版本元数据（`stuntdouble <version>`） | 已实现 |

## 退出码

所有命令都使用这些退出码：

| 码 | 含义 |
| ---: | --- |
| `0` | 成功；`serve` 对正常返回（包括完成 graceful shutdown）使用 `0` |
| `1` | 运行期／服务错误（由 `serve` 报告） |
| `2` | 配置错误或 CLI 用法错误 |
| `3` | 内部错误 |

`serve` 在第二次关闭信号放弃排空时另用 `130`/`143`；见上文“关闭信号”。

## 弃用

- 移除或改名 flag 或命令前，至少提前一个 minor 版本警告。
- 警告写入 stderr 与 `CHANGELOG.md`。
- `1.0` 及以后按 SemVer 保证 CLI 兼容性。

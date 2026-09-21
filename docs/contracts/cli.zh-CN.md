# CLI 契约

[English](./cli.md) \| **中文**

> 本页是英文版 [cli.md](./cli.md) 的译本；如有出入，以英文版为准。

- **状态**：v0.x 切片 T6 稳定
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

启动 mock server 并绑定到配置的地址。命中的 Route 在内置 Boa 运行时中执行其 JavaScript，宿主注入 `ctx`，包含 allowlist 约束的 `ctx.http.get` 调用与流式 `ctx.http.pipe` 调用；未命中的 Route 返回 404 `not_found`。脚本失败返回 500 `script_error` 或 `script_no_response`；未捕获的上游传输层失败返回 502 `upstream_unreachable`。`--verbose` 目前接受但不会输出额外诊断；完整 detail 载荷在后续切片落地。

默认配置路径是 `stuntdouble.toml`。配置必须包含 `[[routes]]`；每条 Route 指定 method、path、脚本位置与可选 name。

#### 退出码

- `0`：成功启动（服务运行直到收到关闭信号）
- `2`：配置错误（TOML 非法或 schema 违规）——消息打印到 stderr
- `3`：内部／服务启动失败

#### 绑定语义

`server.bind` 字段必须是 IP 字面量（`127.0.0.1` 等）。hostname 由操作系统解析，本切片不直接支持。省略 `server.port` 时默认 3000。

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

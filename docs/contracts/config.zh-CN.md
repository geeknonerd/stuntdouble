# 配置契约

[English](./config.md) | **中文**

> 本页是英文版 [config.md](./config.md) 的译本；如有出入，以英文版为准。

- **状态**：v0.x 切片 T6 稳定；`config_version` 为将来的迁移预留
- **适用**：v0.1.0-alpha.1 及以后、同一配置族内
- **稳定性**：1.0 之前允许带弃用窗口的破坏性变更

## 格式

主配置格式是 TOML。默认文件是当前工作目录下的 `stuntdouble.toml`。用 `--config <path>` 指定其他文件。

v1 不接受 YAML 与 JSON。

## 必填字段

每份合法配置都要声明：

```toml
config_version = "1"

[server]
bind = "127.0.0.1"        # 本切片只接受数字 IP 地址
port = 3000              # [1, 65535] 范围内的整数

[files]
root = "./files"         # 必须已存在的目录（相对配置文件的父目录解析）

[sandbox]                # 可选；整个表都可省略
script_timeout_ms = 10000 # 正整数；默认 10000

[upstream]               # 可选；整个表都可省略
allow_hosts = ["metadata.example.com"] # 精确 host allowlist；默认 []
timeout_ms = 15000       # 正整数；默认 15000

[[routes]]
name = "manifest"       # 可选，用于日志与诊断的名字
method = "GET"          # 取值之一：GET、POST、PUT、DELETE、PATCH、HEAD、OPTIONS
path = "/demo/..."      # 以 '/' 开头的路径，支持 :param 段
script = "scripts/x.js" # 相对或绝对路径；接受 .js/.mjs/.cjs
```

所有已知键必须属于 `{config_version, server, files, sandbox, upstream, routes}`，未知顶层键会导致校验错误（fail-closed）。`routes[]` 表内的字段必须是 `{name, method, path, script}` 的子集；`[sandbox]` 只接受 `{script_timeout_ms}`，且为正整数（0 被拒绝）。`[upstream]` 只接受 `{allow_hosts, timeout_ms}`：`allow_hosts` 是 host 名或 IP 字面量的数组，使用 URL host 语法（默认 `[]`，即拒绝所有 host）；条目不带 scheme 与端口；域名会被小写化／punycode 化，IPv6 字面量写成方括号形式（`"[::1]"`），加载时归一化为不带方括号的形式。Host 匹配大小写不敏感且不含端口，IP 字面量与 `localhost` 必须显式列出。`timeout_ms` 是正整数（默认 15000）。`script` 必须以 `.js`、`.mjs` 或 `.cjs` 结尾；`.py` 会被拒绝，并给出明确的 "not supported in this slice" 消息。

## 路由执行模型

Route 遵循唯一流水线：`match → source → transform → response`。本切片实现 match 与脚本 transform：命中的 Route 运行其 JavaScript，脚本通过 `ctx.respond` 产生响应。未命中的请求返回 404 `not_found`；脚本抛错、超时或加载失败返回 500 `script_error`；脚本结束却没有调用 `ctx.respond` 时返回 500 `script_no_response`。本切片新增 `ctx.http.get` 与 `ctx.http.pipe`：对 `ctx.http.get` 来说上游响应（含 4xx/5xx）是数据，而 `ctx.http.pipe` 把上游 2xx body 流式转发给客户端，并对最终非 2xx 响应抛出可捕获的 `upstream_http_error`；未捕获的传输层失败返回 502 `upstream_unreachable`。本地静态文件读取与上传在后续切片落地。

### 匹配语义

- method 大小写不敏感，归一化为大写。
- 路径按 '/' 切分为段；`:name` 捕获一段为 `{name: value}`。
- query string 不参与匹配。
- 先声明者优先；本切片不支持通配符与正则。

## 校验

`stuntdouble validate` 报告：

- 出错的文件路径
- 点号字段路径（例如 `routes[0].method`）
- 期望形状与实际值
- TOML 解析器能提供时给出行列号

配置错误以退出码 `2` 结束。

## 弃用

- 移除或改名字段前，至少提前一个 minor 版本警告。
- 警告写入服务端日志与 `CHANGELOG.md`。
- 破坏性变更要在 release notes 中附带迁移示例。
- `1.0` 及以后按 SemVer 保证配置兼容性。

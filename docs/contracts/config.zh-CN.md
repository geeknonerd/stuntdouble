# 配置契约

[English](./config.md) \| **中文**

> 本页是英文版 [config.md](./config.md) 的译本；如有出入，以英文版为准。

- **状态**：v1 切片（T1–T8）已冻结
- **适用**：声明 `config_version = "1"` 的配置
- **稳定性**：版本 `"1"` 内可以新增字段；1.0 之前的破坏性变更需带弃用窗口

## 格式

主配置格式是 TOML。默认文件是当前工作目录下的 `stuntdouble.toml`。用 `--config <path>` 指定其他文件。

v1 不接受 YAML 与 JSON。

## Schema

每份合法配置都是一个 TOML table。未知键、未知 `config_version`、错误的值的形状与缺失的必填字段都会 fail-closed，作为配置错误处理。

### 顶层键

| 键 | 必填 | 类型 | 默认值 | 校验 |
| --- | --- | --- | --- | --- |
| `config_version` | 是 | string | — | 必须恰好是 `"1"`；其他值以退出码 `2` 结束 |
| `server` | 是 | table | — | 只接受 `{bind, port}` |
| `server.bind` | 否 | string | `"127.0.0.1"` | IP 地址字面量；hostname 在校验期被拒绝 |
| `server.port` | 否 | integer | `3000` | `[1, 65535]` 范围内的整数 |
| `files` | 是 | table | — | 只接受 `{root}` |
| `files.root` | 是 | string | — | 必须已存在的目录，相对配置文件解析 |
| `sandbox` | 否 | table | — | 只接受 `{script_timeout_ms}` |
| `sandbox.script_timeout_ms` | 否 | integer | `10000` | 正整数（0 被拒绝） |
| `upstream` | 否 | table | — | 只接受 `{allow_hosts, timeout_ms}` |
| `upstream.allow_hosts` | 否 | string 数组 | `[]` | 使用 URL host 语法；条目不带 scheme 与端口，默认拒绝所有 host |
| `upstream.timeout_ms` | 否 | integer | `15000` | 正整数（0 被拒绝） |
| `routes` | 是 | table 数组 | — | 至少包含一个 Route table；`routes = []` 非法 |

`upstream.allow_hosts` 中的域名会被小写化／punycode 化。IPv6 字面量写成方括号形式（`"[::1]"`），加载时归一化为不带方括号的形式。Host 匹配大小写不敏感且不含端口，IP 字面量与 `localhost` 必须显式列出。

### Route schema

| 字段 | 必填 | 类型 | 校验 |
| --- | --- | --- | --- |
| `name` | 否 | string | 非空；用于日志与诊断，省略时使用 `path` |
| `method` | 是 | string | 取值之一：`GET`、`POST`、`PUT`、`DELETE`、`PATCH`、`HEAD`、`OPTIONS`（大小写不敏感，归一化为大写） |
| `path` | 是 | string | 以 `/` 开头的绝对路径；`:param` 段按名捕获 |
| `script` | 是 | string | 以 `.js`、`.mjs` 或 `.cjs` 结尾；相对路径按配置文件父目录解析，也接受绝对路径 |

`validate` 会检查 `script` 扩展名，但不会读取脚本文件。文件在每次请求命中 Route 时加载；文件缺失或无法读取时，请求返回 500 `script_error`。

## 示例

```toml
config_version = "1"

[server]
bind = "127.0.0.1"        # IP 字面量
port = 3000               # [1, 65535] 范围内的整数

[files]
root = "./files"          # 必须已存在的目录

[sandbox]                 # 可选表
script_timeout_ms = 10000 # 正整数；默认 10000

[upstream]                # 可选表
allow_hosts = ["metadata.example.com"] # 精确 host allowlist；默认 []
timeout_ms = 15000        # 正整数；默认 15000

[[routes]]
name = "manifest"         # 可选
method = "GET"            # 取值之一：GET、POST、PUT、DELETE、PATCH、HEAD、OPTIONS
path = "/demo/..."        # 以 '/' 开头的路径，支持 :param 段
script = "scripts/x.js"   # .js/.mjs/.cjs；相对或绝对路径
```

## Route 执行模型

Route 遵循唯一流水线：`match → source → transform → response`。本切片实现 Match 与脚本 Transform：命中的 Route 运行其 JavaScript，脚本通过 `ctx.respond` 或 `ctx.http.pipe` 产生 Response。未命中的请求返回 404 `not_found`；脚本抛错、超时或加载失败返回 500 `script_error`；脚本结束却没有产生 Response 时返回 500 `script_no_response`。`ctx.http.get` 把上游响应（含 4xx/5xx）视为数据；`ctx.http.pipe` 把上游 2xx Response 流式转发给客户端，并对最终非 2xx 响应抛出可捕获的 `upstream_http_error`；未捕获的上游传输层失败返回 502 `upstream_unreachable`。本地静态文件读取与上传在后续切片落地。

### 匹配语义

- method 匹配大小写不敏感，归一化为大写。
- 路径按 `/` 切分为段；`:name` 捕获一段为 `{name: value}`。
- query string 不参与匹配。
- 先声明者优先；本切片不支持通配符与正则。

## 校验

`stuntdouble validate` 报告：

- 配置文件路径
- 点号字段路径（例如 `routes[0].method`）
- 期望形状与实际值
- TOML 解析器能提供时给出行列号

校验在每一层 table 上对未知键 fail-closed。它还会拒绝未知 `config_version`、非 IP 的 `server.bind`、空 `routes` 数组、非正数 timeout，以及缺失或不是目录的 `files.root`。`validate` 不打开 socket，也不读取 Route 脚本。

配置错误以退出码 `2` 结束。

## 弃用

- 移除或改名字段前，至少提前一个 minor 版本警告。
- 警告写入服务端日志与 `CHANGELOG.md`。
- 破坏性变更要在 release notes 中附带迁移示例。
- `1.0` 及以后按 SemVer 保证配置兼容性。

# 示例文档清单与二进制下载场景

> 本文记录一个已验证的公开演示场景：从元数据接口生成 CSV 清单并下载 PDF。

## 1. 文档目的

本文整理当前已确认的 Mock 接口需求、外部数据依赖、请求处理流程、json-server middleware 实现、启动方式、错误约定、测试结果和运行注意事项。

实现基于 `json-server@0.17.4`，通过 CLI 的 `--middlewares` 参数加载自定义 Express middleware。Mock 服务对外提供两个固定路径，同时保留 json-server 的默认资源能力。

## 2. 需求概述

### 2.1 对外提供的接口

Mock 服务需要实现以下两个 GET 接口：

```text
GET /demo/documents/manifest/:group
GET /demo/documents/download/:document_id
```

这两个接口的返回内容不是直接读取本地 `db.json`，而是依赖一个固定的元数据接口。middleware 每次收到请求时，都会直接访问该元数据接口，不使用缓存，也不使用代理。

### 2.2 固定元数据接口

默认地址：

```text
https://metadata.example.com/demo/documents
```

可以使用环境变量覆盖默认值：

```bash
METADATA_API_URL=https://metadata.example.com/demo/documents
```

元数据接口不需要 API Key。客户端请求中即使携带 `X-Request-ID: key`，middleware 也不会将该请求头转发给元数据接口。

预期响应结构：

```json
{
  "data": [
    {
      "code": "DOC-0001",
      "title": "示例设备 A 安装手册",
      "system_code": "SYS-A",
      "pdf_url": "http://files.example.com/demo/documents/DOC-0001.pdf"
    },
    {
      "code": "DOC-0002",
      "title": "示例设备 B 运行手册",
      "system_code": "SYS-B",
      "pdf_url": "http://files.example.com/demo/documents/DOC-0002.pdf"
    }
  ]
}
```

`data` 必须是数组。数组元素至少应包含：

| 字段 | 用途 | 是否用于 manifest | 是否用于 download |
| --- | --- | ---: | ---: |
| `code` | 文档编码/唯一匹配值 | 是 | 是，和 `:document_id` 精确匹配 |
| `title` | 文件标题 | 是 | 否 |
| `system_code` | 系统代码 | 是 | 否 |
| `pdf_url` | PDF 下载地址 | 否 | 是 |

## 3. 接口契约

### 3.0 原始调用与当前 Mock 的关系

需求最初给出的调用示例是直接访问 公开示例 Mock：

```bash
curl -k -o 示例文档清单.csv \
  -H 'X-Request-ID: key' \
  https://metadata.example.com/demo/documents/manifest/group-a

curl -k -o DOC-0003.pdf \
  -H 'X-Request-ID: key' \
  https://metadata.example.com/demo/documents/download/DOC-0003
```

当前实现不是简单地把上述 公开示例 路径原样代理到本地，而是在本地 json-server 上重建同名业务路径：

```text
本地 Mock:  http://<host>:3000/demo/documents/manifest/group-a
本地 Mock:  http://<host>:3000/demo/documents/download/<document_id>
```

本地 middleware 直接请求固定元数据接口，再完成字段转换和 PDF 下载。客户端可以继续保留 `X-Request-ID` 以兼容既有调用代码，但该 header 不会被使用或转发。元数据和 PDF 请求均为 Node.js 直接网络请求，不使用代理。

### 3.1 获取清单：`GET /demo/documents/manifest/:group`

#### 路径参数

| 参数 | 说明 |
| --- | --- |
| `plant` | 为兼容既有调用方而保留的路径参数，例如 `group-a`；当前实现不使用它筛选数据。无论传入什么值，都会输出元数据接口 `data` 数组中的全部记录。 |

#### 请求示例

```bash
curl -i \
  http://localhost:3000/demo/documents/manifest/group-a
```

客户端可以附加 `X-Request-ID`，但当前实现不会使用或转发该 header：

```bash
curl -i \
  -H 'X-Request-ID: key' \
  http://localhost:3000/demo/documents/manifest/group-a
```

#### 成功响应

状态码：`200 OK`

响应头：

```http
Content-Type: text/plain; charset=utf-8
```

响应体是普通 UTF-8 文本，内容采用 CSV 行格式：

```text
文件编码,文件标题,系统代码
DOC-0001,示例设备 A 安装手册,SYS-A
DOC-0002,示例设备 B 运行手册,SYS-B
```

实现规则：

1. 请求固定元数据接口。
2. 读取顶层 `data` 数组。
3. 每条记录按 `code,title,system_code` 顺序生成一行。
4. 第一行固定输出中文表头：`文件编码,文件标题,系统代码`。
5. 字段包含逗号、双引号、回车或换行时，按 CSV 规则包裹双引号，并将字段内部的双引号替换为两个双引号。
6. 响应结尾带一个换行符。
7. `pdf_url` 不会出现在清单响应中。

### 3.2 下载 PDF：`GET /demo/documents/download/:document_id`

#### 路径参数

| 参数 | 说明 |
| --- | --- |
| `document_id` | 与元数据数组元素的 `code` 字段比较。当前实现使用字符串精确匹配：`String(item.code) === document_id`。 |

#### 请求示例

```bash
curl -fSLo DOC-0001.pdf \
  http://localhost:3000/demo/documents/download/DOC-0001
```

#### 成功响应

状态码：`200 OK`

响应头：

```http
Content-Type: application/pdf
Content-Length: <实际字节数>
Content-Disposition: attachment; filename="DOC-0001.pdf"
```

响应体是从匹配记录的 `pdf_url` 下载得到的 PDF 二进制内容。当前实现允许 `pdf_url` 指向公网、局域网或本机文件服务，只校验 URL 协议必须是 `http://` 或 `https://`。

下载流程：

1. 请求固定元数据接口。
2. 在 `data` 数组中查找 `code === :document_id`。
3. 找不到匹配项时直接返回 `404`，不会请求任何 PDF 地址。
4. 对 `pdf_url` 做 URL 解析和协议校验。
5. 请求 PDF 地址；请求采用手动重定向模式。
6. 对每个重定向目标再次校验 URL 协议，最多跟随 3 次重定向。
7. 将响应体读为 Buffer，并以 `application/pdf` 返回给客户端。

## 4. 状态码与错误响应

默认错误响应为 JSON，且不把底层异常堆栈或上游响应体返回给客户端。

| 场景 | HTTP 状态 | 响应示例 |
| --- | ---: | --- |
| `document_id` 在元数据 `data` 中不存在 | `404` | `{"error":"document_not_found"}` |
| 元数据接口不可达、超时或返回非 2xx | `502` | `{"error":"metadata_bad_gateway"}` |
| 元数据响应不是合法 JSON | `502` | `{"error":"metadata_bad_gateway"}` |
| 元数据顶层没有 `data` 数组 | `502` | `{"error":"metadata_bad_gateway"}` |
| `pdf_url` 缺失、格式非法或协议不是 HTTP/HTTPS | `502` | `{"error":"pdf_url_invalid"}` |
| PDF 地址请求失败、返回非 2xx、重定向缺少 Location 或超过 3 跳 | `502` | `{"error":"pdf_bad_gateway"}` |

### 4.1 诊断模式

启动时设置：

```bash
DEBUG_MOCK=1 npm start
```

诊断模式会：

- 在服务端 stderr 输出带 `[mock-debug]` 前缀的阶段日志；
- 在 502 JSON 中额外加入 `detail` 字段；
- 不返回上游响应体，避免把敏感信息泄露给调用方。

示例：

```json
{
  "error": "metadata_bad_gateway",
  "detail": "metadata_upstream_failed_404"
}
```

共享或生产环境不建议开启 `DEBUG_MOCK=1`，因为 `detail` 可能暴露内部网络错误信息。

## 5. 运行架构

```text
客户端
  |
  | GET /demo/documents/manifest/group-a
  | GET /demo/documents/download/{document_id}
  v
json-server (Express, 0.0.0.0:3000)
  |
  +--> middleware.js
  |      |
  |      +--> 固定元数据接口
  |      |       返回 { data: [...] }
  |      |
  |      +--> manifest: 生成 text/plain CSV
  |      |
  |      +--> download: code 匹配 -> pdf_url -> 下载 PDF
  |
  +--> json-server router(db.json)
          处理其他未被 middleware 截获的资源请求
```

middleware 只处理：

- `GET /demo/documents/manifest/<非空路径段>`
- `GET /demo/documents/download/<非空路径段>`

其他 HTTP 方法和其他路径调用 `next()`，继续交给 json-server 的后续 middleware/router。

## 6. 文件结构与职责

以下文件树描述实现代码所在的外部目录；本仓库只保存调研与实现文档，不包含该实现代码。

```text
<stuntdouble-implementation-dir>/
├── middleware.js
├── db.json
├── package.json
├── README.md
├── test/
│   └── middleware.test.js
```

### `middleware.js`

主要职责：

- 导出 json-server CLI 可加载的 CommonJS middleware 函数；
- 读取 `METADATA_API_URL`；
- 请求并校验元数据响应；
- 生成清单文本；
- 按文档编码查找 PDF 地址；
- 下载并返回 PDF Buffer；
- 处理超时、错误映射、协议校验和有限重定向；
- 提供 `createMiddleware()` 工厂，便于测试注入 fetch 实现。

### `db.json`

当前内容为空对象 `{}`。它只是 json-server CLI 的数据源占位文件；两个文档接口的数据不从此文件读取。

### `package.json`

关键配置：

```json
{
  "scripts": {
    "start": "json-server db.json --middlewares ./middleware.js --host 0.0.0.0 --port 3000",
    "test": "node --test"
  },
  "engines": {
    "node": ">=18"
  },
  "dependencies": {
    "json-server": "0.17.4"
  }
}
```

Node.js 18 或更新版本提供全局 `fetch` 和 `AbortController`，是当前实现的运行前提。

## 7. 安装与启动

### 7.1 安装依赖

以下命令在实现代码所在的外部目录执行：

```bash
cd <stuntdouble-implementation-dir>
npm install
```

### 7.2 使用默认元数据地址启动

```bash
npm start
```

等价命令：

```bash
npx json-server@0.17.4 db.json \
  --middlewares ./middleware.js \
  --host 0.0.0.0 \
  --port 3000
```

启动日志通常包含：

```text
Loading db.json
Loading ./middleware.js
Done
Home
http://0.0.0.0:3000
```

### 7.3 覆盖元数据地址

macOS/Linux：

```bash
METADATA_API_URL='https://metadata.example.com/demo/documents' npm start
```

Windows PowerShell：

```powershell
$env:METADATA_API_URL = 'https://metadata.example.com/demo/documents'
npm start
```

### 7.4 调整上游超时

默认超时为 15 秒，单位为毫秒：

```bash
UPSTREAM_TIMEOUT_MS=30000 npm start
```

无效或不大于 0 的值会回退到 15 秒。

### 7.5 局域网访问

服务使用 `--host 0.0.0.0` 监听所有 IPv4 网卡。局域网客户端必须访问运行主机的实际局域网 IP，而不是把 `0.0.0.0` 当作目标地址：

```bash
curl http://192.0.2.10:3000/demo/documents/manifest/group-a
```

请将 `192.0.2.10` 替换为实际 IP，并确保：

1. 运行主机防火墙允许可信局域网访问 TCP 3000；
2. 网络策略没有阻止客户端到服务端的连接；
3. 服务端确实以 `--host 0.0.0.0` 启动；
4. 客户端不要访问 `http://0.0.0.0:3000`。

`0.0.0.0` 会扩大暴露面，不应直接暴露到不受信任网络。

## 8. 测试与验证

执行：

```bash
npm test
```

当前测试文件为 `test/middleware.test.js`，覆盖：

1. manifest 从 `{ data: [...] }` 生成纯文本 CSV；
2. CSV 字段包含逗号时的转义；
3. download 按 `code` 匹配并返回 PDF Buffer；
4. 元数据中不存在文档时返回 404；
5. 元数据上游非 2xx 时返回 502；
6. `DEBUG_MOCK=1` 时返回诊断 detail；
7. 元数据 URL 空白和无效超时配置的处理；
8. 非目标请求继续执行 `next()`；
9. 内网 PDF 地址可以正常下载；
10. PDF 安全重定向可以被跟随。

最近一次验证结果：

```text
9 tests passed
```

此外，`node --check middleware.js` 和 `node --check test/middleware.test.js` 均通过。

## 9. 故障排查手册

### 9.1 `curl: (7) Failed to connect to 127.0.0.1 port 3000`

先确认服务是否仍在运行：

```bash
lsof -nP -iTCP:3000 -sTCP:LISTEN
```

启动命令必须包含：

```bash
--host 0.0.0.0 --port 3000
```

如果只绑定到 IPv6 的 `[::1]:3000`，IPv4 客户端访问 `127.0.0.1:3000` 可能失败。重新以 `--host 0.0.0.0` 启动。

### 9.2 下载返回 502 且响应长度约 32 字节

```json
{"error":"metadata_bad_gateway"}
```

这表示失败发生在元数据请求或解析阶段，还没开始下载 PDF。开启 `DEBUG_MOCK=1` 查看 `detail`，并直接检查元数据接口：

```bash
export METADATA_API_URL='https://metadata.example.com/demo/documents'
curl -vS --max-time 20 "$METADATA_API_URL"
```

应确认：

- HTTP 状态是 2xx；
- 响应是合法 JSON；
- 顶层存在 `data` 数组。

### 9.3 下载返回 `pdf_url_invalid`

当前实现只会因以下原因返回该错误：

- `pdf_url` 为空或无法被 `new URL(...)` 解析；
- URL 协议不是 `http:` 或 `https:`。

内网、本机、局域网 IP 已被允许，不会再因为私网地址本身被拒绝。

### 9.4 下载返回 `pdf_bad_gateway`

说明元数据已成功读取并完成文档匹配，但 PDF 下载阶段失败。常见原因：

- PDF 主机不可达；
- PDF 地址返回非 2xx；
- 重定向缺少 `Location`；
- 重定向超过 3 跳；
- 上游响应体读取失败；
- 请求超过 `UPSTREAM_TIMEOUT_MS`。

使用 `DEBUG_MOCK=1` 重新请求可以看到内部错误 detail。

## 10. 当前实现的边界与注意事项

### 10.1 不缓存

manifest 和 download 每次请求都重新请求元数据接口。优点是数据和 `pdf_url` 始终取最新值；缺点是增加上游依赖和延迟。若未来需要缓存，应明确 TTL、失效策略和测试环境可重复性要求。

### 10.2 PDF 使用内存 Buffer

当前实现使用 `await response.arrayBuffer()` 后转换为 Buffer，再一次性发送。适合中小型 PDF；大文件或高并发场景建议改为真正的流式转发，以降低内存峰值。

### 10.3 允许内网 URL 的风险

当前需求明确要求允许内网文件服务，因此 middleware 不再阻止私网、本机或局域网地址。必须保证元数据接口可信，且不要让不受信任的用户直接控制 `pdf_url`，否则服务可能被用于访问任意内部 HTTP 服务（SSRF）。

### 10.4 TLS 证书

Node.js `fetch` 默认校验证书。客户端的 `curl -k` 只会关闭 curl 自身的证书校验，不会关闭 Node.js 对上游 PDF 或元数据接口的校验。应在运行环境安装正确的 CA 证书，不建议通过 `NODE_TLS_REJECT_UNAUTHORIZED=0` 全局关闭校验。

### 10.5 API Key 行为

当前实现不读取客户端 `X-Request-ID`，也不向元数据接口注入 API Key。如果未来上游接口改为需要认证，应新增服务端环境变量和固定请求头配置，不要透传客户端凭据。

### 10.6 json-server 版本

当前锁定 `json-server@0.17.4`，使用 CommonJS middleware 导出和 `--middlewares` CLI 参数。若升级到 v1 主线或其他版本，应重新核对 CLI 参数、模块导出和 middleware 加载行为。

## 11. 交付清单

启动并交付该 Mock 服务至少需要以下文件：

- `middleware.js`
- `db.json`
- `package.json`

建议同时交付：

- `README.md`：快速启动和调用说明；
- `test/middleware.test.js`：离线行为测试；
- 本文档：完整需求、契约和运维说明。

## 12. 最终结论

当前实现满足以下目标：

1. 使用 json-server CLI 加载自定义 middleware；
2. 对外提供 `/demo/documents/manifest/:group` 清单接口；
3. 清单接口从固定元数据接口读取 `data`，生成普通 `text/plain` CSV 文本；
4. 对外提供 `/demo/documents/download/:document_id` 下载接口；
5. 下载接口按 `code` 匹配元数据，读取 `pdf_url`，获取并返回 PDF 文件；
6. 支持公网、局域网和本机 PDF 文件服务；
7. 通过 `--host 0.0.0.0` 支持局域网客户端访问；
8. 对上游失败、文档不存在、URL 非法和 PDF 下载失败提供稳定状态码；
9. 通过测试和 `DEBUG_MOCK=1` 诊断机制支持本地验证与故障定位。

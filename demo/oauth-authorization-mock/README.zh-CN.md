# OAuth 授权码 mock

[English](./README.md) | **中文**

> 本页是英文版 [README.md](./README.md) 的译本；如有出入，以英文版为准。

这是 [plans/oauth-authorization-mock.md](../../plans/oauth-authorization-mock.md) 中 OAuth 授权 mock 契约的可运行 Stunt Double 夹具。

此演示实现了一个最小化的 OAuth 2.0 authorization code flow mock，用于在集成测试中验证客户端行为。它签发一次性授权码、兑换访问令牌、返回固定用户信息——全部无需外部网络访问、真实凭据或持久化存储。

## 路由

| 方法 | 路径 | 描述 |
|------|------|------|
| `GET` | `/oauth2/authorize` | 授权端点——验证请求参数并签发一次性授权码 |
| `POST` | `/oauth2/token` | 令牌端点——用有效授权码兑换访问令牌（需要请求签名） |
| `GET` | `/oauth2/userinfo` | 用户信息端点——返回固定测试用户数据（需要请求签名和有效访问令牌） |

## 运行

```bash
stuntdouble serve --config demo/stuntdouble.toml
```

演示共用 `demo/stuntdouble.toml`，该配置绑定 `127.0.0.1:3000`。回调地址是 `http://127.0.0.1:18081/callback`（并无服务器在 18081 监听，重定向目标只是供客户端读取的固定契约值）。不需要环境变量、`allow_hosts`、外部网络或真实凭据。下面的端到端示例使用端口 `3000`；通过 `cargo test` 运行时，测试套件会为每次运行分配一个空闲端口。

### 前提条件

- `stuntdouble` 二进制（v1.x），从此仓库构建或从发布版本下载。
- 具备 `curl`、`openssl`、`sha256sum`/`shasum` 与 `date` 的 shell，用于下面的签名示例。
- 手动运行时端口 `3000` 在 `127.0.0.1` 上空闲。

```bash
# 第一步：获取授权码
curl -i "http://127.0.0.1:3000/oauth2/authorize?response_type=code&client_id=demo-client&redirect_uri=http%3A%2F%2F127.0.0.1%3A18081%2Fcallback&state=state-123"

# 第二步：跟随重定向从 Location 头提取 code
# 第三步：用 code 兑换访问令牌（需要签名头）
# 第四步：携带访问令牌请求用户信息
```

### 签名计算（令牌与用户信息）

令牌和用户信息端点需要 mock 请求签名，见[契约](../../plans/oauth-authorization-mock.md)第 6 节。签名原文如下：

```
METHOD
PATH
CANONICAL_QUERY
TIMESTAMP
REQUEST_ID
SHA256_HEX(BODY)
```

头部 `X-Sig-Key-Id`、`X-Sig-Request-Id`、`X-Sig-Timestamp`、`X-Sig-Version`、`X-Sig-Signature`；key id `mock-key-01`，版本 `1`，密钥 `demo-signing-secret`。`X-Sig-Request-Id` 为 32 个小写十六进制字符；`X-Sig-Timestamp` 为 Unix 秒，与服务器时间相差不得超过 ±300 秒；同一请求 ID 在 10 分钟内只能使用一次。

以下是完整的端到端示例（bash），先获取授权码，再换取令牌，最后读取用户信息：

```bash
set -euo pipefail

BASE="http://127.0.0.1:3000"
SECRET="demo-signing-secret"
REDIRECT="http://127.0.0.1:18081/callback"

# 1. Authorize
AUTH="$BASE/oauth2/authorize?response_type=code&client_id=demo-client&redirect_uri=$(printf '%s' "$REDIRECT" | openssl dgst -sha256 >/dev/null && printf '%s' "$REDIRECT" | jq -sRr @uri 2>/dev/null || printf '%s' "$REDIRECT" | sed 's/:/%3A/g; s/\//%2F/g')&state=state-123"
LOCATION=$(curl -s -D - "$AUTH" | awk -F': ' 'tolower($1)=="location"{print $2}' | tr -d '\r')
CODE=$(printf '%s' "$LOCATION" | sed -n 's/.*code=\([^&]*\).*//p')
echo "code=$CODE"

# 2. Token
BODY="grant_type=authorization_code&client_id=demo-client&client_secret=demo-client-secret&code=$CODE&redirect_uri=$(printf '%s' "$REDIRECT" | sed 's/:/%3A/g; s/\//%2F/g')"
TIMESTAMP=$(date +%s)
REQUEST_ID=$(openssl rand -hex 16)
BODY_SHA256=$(printf '%s' "$BODY" | sha256sum | cut -d' ' -f1)
CANONICAL="POST
/oauth2/token

${TIMESTAMP}
${REQUEST_ID}
${BODY_SHA256}"
SIGNATURE=$(printf '%s' "$CANONICAL" | openssl dgst -sha256 -hmac "$SECRET" -hex | sed 's/^.*= //')

TOKEN_RESPONSE=$(curl -s -D - -X POST "$BASE/oauth2/token"   -H 'Content-Type: application/x-www-form-urlencoded'   -H "X-Sig-Key-Id: mock-key-01"   -H "X-Sig-Request-Id: $REQUEST_ID"   -H "X-Sig-Timestamp: $TIMESTAMP"   -H 'X-Sig-Version: 1'   -H "X-Sig-Signature: $SIGNATURE"   --data-binary "$BODY")
TOKEN=$(printf '%s' "$TOKEN_RESPONSE" | awk -F'"access_token":' '{print $2}' | tr -d '"}, \r' | head -c 35)
echo "token=$TOKEN"

# 3. Userinfo
TIMESTAMP=$(date +%s)
REQUEST_ID=$(openssl rand -hex 16)
EMPTY_SHA256=$(printf '' | sha256sum | cut -d' ' -f1)
CANONICAL="GET
/oauth2/userinfo

${TIMESTAMP}
${REQUEST_ID}
${EMPTY_SHA256}"
SIGNATURE=$(printf '%s' "$CANONICAL" | openssl dgst -sha256 -hmac "$SECRET" -hex | sed 's/^.*= //')

curl -i "$BASE/oauth2/userinfo"   -H "Authorization: Bearer $TOKEN"   -H 'X-Sig-Key-Id: mock-key-01'   -H "X-Sig-Request-Id: $REQUEST_ID"   -H "X-Sig-Timestamp: $TIMESTAMP"   -H 'X-Sig-Version: 1'   -H "X-Sig-Signature: $SIGNATURE"
```

上述签名示例为可移植性避开了 `jq`；如你愿意，可将百分号编码行替换为 `jq -sRr @uri`。所有值（`demo-client`、`demo-client-secret`、`mock-key-01`、`demo-signing-secret`、`u-10001`、`Alice`）均为固定测试夹具值，非真实凭据。

### 回调处理

授权端点返回带 `Location` 头的 `302`：

```
Location: http://127.0.0.1:18081/callback?code=<64_HEX>&state=<ENCODED_STATE>
```

客户端跟随重定向（或不跟随直接解析 `Location`），提取 `code` 查询参数。没有服务器在 18081 上监听；回调 URL 是固定契约值。对 `state` 做百分号解码还原原始值，并与你发起授权流时发送的值比对，以将浏览器流与本次响应绑定。

### 预期响应

| 步骤 | 端点 | 成功状态码 | 成功 body |
|------|------|------------|-----------|
| 授权 | `GET /oauth2/authorize` | 302 | 无（`Location` 头） |
| 令牌 | `POST /oauth2/token` | 200 | `{"access_token","token_type":"Bearer","expires_in":300}` |
| 用户信息 | `GET /oauth2/userinfo` | 200 | `{"sub":"u-10001","name":"Alice","org_id":"org-10","org_name":"Example Org"}` |

所有 JSON 响应使用 `Content-Type: application/json; charset=utf-8`，并带 `Cache-Control: no-store` 与 `Pragma: no-cache`。`access_token` 以 `at-` 开头，后跟 32 个小写十六进制字符（共 35 字符），生效期 300 秒。

### 失败情况

- 未知路径 → 404 `{"error":"not_found",...}`。
- 已知路径的错误方法 → 405 `{"error":"invalid_method",...}`，并带宿主注入的 `Allow` 头。
- 授权参数错误 → 400 `invalid_request`；不会发生重定向。
- 签名缺失/格式错误/超出窗口/重放 → 401 `invalid_signature`。
- `client_id`/`client_secret` 错误 → 401 `invalid_client`。
- 授权码无效/过期/已用，或 `redirect_uri` 不匹配 → 400 `invalid_grant`。
- 缺失或错误的 Bearer 令牌 → 401 `invalid_token`。
- 令牌端点非表单 body → 400 `invalid_request`。

绝不会产生 HTML 错误页；每个错误 body 都是只包含 `error` 与 `error_description` 的 JSON。

### 清理

演示不持久化任何状态：授权码、访问令牌与签名请求 ID 带 TTL（授权码 60 秒、访问令牌 300 秒、请求 ID 10 分钟）存放在进程内存中。停止服务器（`Ctrl-C`）后所有内存状态即被清空。不写入任何文件，也不访问外部服务。

### 支持范围与不支持的行为

- **仅支持授权码授权。** `response_type` 必须为 `code`；`grant_type` 必须为 `authorization_code`。
- **不支持：** 用户登录或真实认证、刷新令牌、PKCE、OIDC 发现、JWKS、ID 令牌、登出/会话管理、动态客户端注册、多租户隔离、跨重启持久化、多实例协调。
- 本演示不实现完整的 OAuth 2.0 或 OIDC 兼容性；它只是用于验证客户端集成行为的测试夹具。

## 固定测试数据

| 字段 | 值 |
|------|-----|
| `client_id` | `demo-client` |
| `client_secret` | `demo-client-secret` |
| 允许的回调地址 | `http://127.0.0.1:18081/callback` |
| 测试用户 `sub` | `u-10001` |
| 测试用户 `name` | `Alice` |

## 授权端点契约

`GET /oauth2/authorize` 验证以下查询参数：

- `response_type` 必须为 `code`
- `client_id` 必须为 `demo-client`
- `redirect_uri` 必须精确匹配 `http://127.0.0.1:18081/callback`
- `state` 必须是 1–128 个可见 ASCII 字符（0x21–0x7e）

成功时，端点返回 `302`，`Location` 头包含：
- `code`：通过 `ctx.crypto.randomBytes` 生成的 32 字节随机数转换而来的 64 位小写十六进制授权码
- `state`：原始 state 值，使用大写十六进制百分号编码；仅 RFC 3986 unreserved
  字符保持原样

响应包含 `Cache-Control: no-store`。

验证失败时，端点返回 `400`，JSON body：
```json
{"error":"invalid_request","error_description":"missing or invalid parameter"}
```

授权码通过 `ctx.state` 存储，TTL 为 60 秒。授权码绝不接受来自客户端的输入。

授权端点不需要也不读取请求签名头（`X-Sig-*`）。

## 文件

- `demo/scripts/authorize.js` —— 授权端点脚本
- `demo/scripts/token.js` —— 令牌端点脚本
- `demo/scripts/userinfo.js` —— 用户信息端点脚本
- `demo/scripts/http-errors.js` —— 兜底路由脚本，处理未映射路径（404）与不支持的方法（405）
- `demo/stuntdouble.toml` —— 演示夹具的路由声明

## 测试

```bash
cargo test --test cli oauth_
```

覆盖[契约](../../plans/oauth-authorization-mock.md) 中的 A01–A20：授权参数校验与 state 边界、令牌兑换（A05–A14）、用户信息（A15–A18：Bearer 校验、签名优先于令牌、令牌过期、精确响应结构、重放拒绝、规范查询串签名、日志卫生）和兜底响应（A19–A20：404 `not_found`、405 `invalid_method` 携带 `Allow`）。

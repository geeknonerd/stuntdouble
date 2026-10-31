# OAuth 授权码 mock

[English](./README.md) | **中文**

> 本页是英文版 [README.md](./README.md) 的译本；如有出入，以英文版为准。

这是 [plans/oauth-authorization-mock.md](../../plans/oauth-authorization-mock.md) 中 OAuth 授权 mock 契约的可运行 Stunt Double 夹具。

此演示实现了一个最小化的 OAuth 2.0 authorization code flow mock，用于在集成测试中验证客户端行为。它签发一次性授权码、兑换访问令牌、返回固定用户信息——全部无需外部网络访问或真实凭据。

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

授权端点不需要环境变量或 `allow_hosts`。令牌和用户信息端点使用 mock 自身的请求签名方案。

```bash
# 第一步：获取授权码
curl -i "http://127.0.0.1:18080/oauth2/authorize?response_type=code&client_id=demo-client&redirect_uri=http%3A%2F%2F127.0.0.1%3A18081%2Fcallback&state=state-123"

# 第二步：跟随重定向从 Location 头提取 code
# 第三步：用 code 兑换访问令牌（需要签名头）
# 第四步：携带访问令牌请求用户信息
```

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
- `demo/stuntdouble.toml` —— 演示夹具的路由声明

## 测试

```bash
cargo test --test cli oauth_
```

覆盖授权参数校验与 state 边界，以及 A05–A14 令牌兑换：仅接受表单、签名优先级与规范查询串、重放与授权码过期、原子单次消费、客户端与回调地址校验和响应结构。

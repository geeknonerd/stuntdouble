# OAuth authorization code mock

**English** | [中文](./README.zh-CN.md)

Runnable Stunt Double fixture for the [OAuth authorization mock contract](../../plans/oauth-authorization-mock.md).

This demo implements a minimal OAuth 2.0 authorization code flow mock for verifying client behavior during integration testing. It issues one-time authorization codes, exchanges them for access tokens, and returns fixed user info — all without external network access or real credentials.

## Routes

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/oauth2/authorize` | Authorization endpoint — validates request parameters and issues a one-time authorization code |
| `POST` | `/oauth2/token` | Token endpoint — exchanges a valid authorization code for an access token (requires request signature) |
| `GET` | `/oauth2/userinfo` | Userinfo endpoint — returns fixed test user data (requires request signature and valid access token) |

## Run it

```bash
stuntdouble serve --config demo/stuntdouble.toml
```

No environment variables or `allow_hosts` are required for the authorize endpoint. The token and userinfo endpoints use the mock's own request-signature scheme.

```bash
# Step 1: Get an authorization code
curl -i "http://127.0.0.1:18080/oauth2/authorize?response_type=code&client_id=demo-client&redirect_uri=http%3A%2F%2F127.0.0.1%3A18081%2Fcallback&state=state-123"

# Step 2: Follow the redirect to extract the code from the Location header
# Step 3: Exchange the code for an access token (requires signature headers)
# Step 4: Request userinfo with the access token
```

## Fixed test data

| Field | Value |
|-------|-------|
| `client_id` | `demo-client` |
| `client_secret` | `demo-client-secret` |
| Allowed redirect URI | `http://127.0.0.1:18081/callback` |
| Test user `sub` | `u-10001` |
| Test user `name` | `Alice` |

## Authorize endpoint contract

`GET /oauth2/authorize` validates the following query parameters:

- `response_type` must be `code`
- `client_id` must be `demo-client`
- `redirect_uri` must exactly match `http://127.0.0.1:18081/callback`
- `state` must be 1–128 visible ASCII characters (0x21–0x7e)

On success, the endpoint returns `302` with a `Location` header containing:
- `code`: a 64-character lowercase hexadecimal authorization code generated from 32 random bytes via `ctx.crypto.randomBytes`
- `state`: the original state value, percent-encoded with uppercase hex; only
  RFC 3986 unreserved characters stay literal

The response includes `Cache-Control: no-store`.

On validation failure, the endpoint returns `400` with a JSON body:
```json
{"error":"invalid_request","error_description":"missing or invalid parameter"}
```

The authorization code is stored via `ctx.state` with a 60-second TTL. The code is never accepted from client input.

The authorize endpoint does not require or read request-signature headers (`X-Sig-*`).

## Files

- `demo/scripts/authorize.js` — Authorization endpoint script
- `demo/scripts/token.js` — Token endpoint script
- `demo/stuntdouble.toml` — Route declarations for the demo fixture

## Tests

```bash
cargo test --test cli oauth_
```

Covers authorize parameter validation and state boundaries, plus A05–A14 token exchange: form-only parsing, signature precedence and canonical queries, replay and code expiry, atomic single-use codes, client and redirect validation, and response shape.

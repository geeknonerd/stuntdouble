# OAuth authorization code mock

**English** | [中文](./README.zh-CN.md)

Runnable Stunt Double fixture for the [OAuth authorization mock contract](../../plans/oauth-authorization-mock.md).

This demo implements a minimal OAuth 2.0 authorization code flow mock for verifying client behavior during integration testing. It issues one-time authorization codes, exchanges them for access tokens, and returns fixed user info — all without external network access, real credentials, or persistent storage.

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

The demo shares `demo/stuntdouble.toml`, which binds `127.0.0.1:3000`. Its callback address is `http://127.0.0.1:18081/callback` (no server listens on 18081 — the redirect target is a fixed contract value the client reads). No environment variables, `allow_hosts`, external network, or real credentials are needed. The end-to-end examples below use port `3000`; when run through `cargo test`, the suite allocates a free port per run.

### Prerequisites

- The `stuntdouble` binary (v1.x), built from this repo or downloaded from a release.
- A shell with `curl`, `openssl`, `sha256sum`/`shasum`, and `date` for the signature examples below.
- Port `3000` free on `127.0.0.1` for a manual run.

```bash
# Step 1: Get an authorization code
curl -i "http://127.0.0.1:3000/oauth2/authorize?response_type=code&client_id=demo-client&redirect_uri=http%3A%2F%2F127.0.0.1%3A18081%2Fcallback&state=state-123"

# Step 2: Follow the redirect to extract the code from the Location header
# Step 3: Exchange the code for an access token (requires signature headers)
# Step 4: Request userinfo with the access token
```

### Signature calculation (token and userinfo)

The token and userinfo endpoints require the mock request signature from §6 of the [contract](plans/oauth-authorization-mock.md). Compute it over the canonical string:

```
METHOD
PATH
CANONICAL_QUERY
TIMESTAMP
REQUEST_ID
SHA256_HEX(BODY)
```

Headers `X-Sig-Key-Id`, `X-Sig-Request-Id`, `X-Sig-Timestamp`, `X-Sig-Version`, `X-Sig-Signature`; key id `mock-key-01`, version `1`, secret `demo-signing-secret`. `X-Sig-Request-Id` is 32 lowercase hex chars; `X-Sig-Timestamp` is Unix seconds within ±300s of server time; a request ID may be used once within 10 minutes.

Complete end-to-end example (bash). It fetches a code, exchanges it for a token, then reads userinfo:

```bash
set -euo pipefail

BASE="http://127.0.0.1:3000"
SECRET="demo-signing-secret"
REDIRECT="http://127.0.0.1:18081/callback"

# 1. Authorize
AUTH="$BASE/oauth2/authorize?response_type=code&client_id=demo-client&redirect_uri=$(printf '%s' "$REDIRECT" | openssl dgst -sha256 >/dev/null && printf '%s' "$REDIRECT" | jq -sRr @uri 2>/dev/null || printf '%s' "$REDIRECT" | sed 's/:/%3A/g; s/\//%2F/g')&state=state-123"
LOCATION=$(curl -s -D - "$AUTH" | awk -F': ' 'tolower($1)=="location"{print $2}' | tr -d '\r')
CODE=$(printf '%s' "$LOCATION" | sed -n 's/.*code=\([^&]*\).*/\1/p')
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

TOKEN_RESPONSE=$(curl -s -D - -X POST "$BASE/oauth2/token" \
  -H 'Content-Type: application/x-www-form-urlencoded' \
  -H "X-Sig-Key-Id: mock-key-01" \
  -H "X-Sig-Request-Id: $REQUEST_ID" \
  -H "X-Sig-Timestamp: $TIMESTAMP" \
  -H 'X-Sig-Version: 1' \
  -H "X-Sig-Signature: $SIGNATURE" \
  --data-binary "$BODY")
TOKEN=$(printf '%s' "$TOKEN_RESPONSE" | awk -F'"access_token":' '{print $2}' | tr -d '",} \r' | head -c 35)
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

curl -i "$BASE/oauth2/userinfo" \
  -H "Authorization: Bearer $TOKEN" \
  -H 'X-Sig-Key-Id: mock-key-01' \
  -H "X-Sig-Request-Id: $REQUEST_ID" \
  -H "X-Sig-Timestamp: $TIMESTAMP" \
  -H 'X-Sig-Version: 1' \
  -H "X-Sig-Signature: $SIGNATURE"
```

The signature example above avoids `jq` for portability; replace the percent-encoding line with `jq -sRr @uri` if you prefer. All values (`demo-client`, `demo-client-secret`, `mock-key-01`, `demo-signing-secret`, `u-10001`, `Alice`) are fixed test-only fixture values, not real credentials.

## Callback handling

The authorize endpoint returns `302` with a `Location` header:

```
Location: http://127.0.0.1:18081/callback?code=<64_HEX>&state=<ENCODED_STATE>
```

The client follows the redirect (or parses `Location` without following) and extracts the `code` query parameter. No server listens on 18081; the callback URL is a fixed contract value. Percent-decode `state` to recover the original value and compare it against the value you sent to bind the browser flow to this response.

## Expected responses

| Step | Endpoint | Success status | Success body |
|------|----------|----------------|--------------|
| Authorize | `GET /oauth2/authorize` | 302 | none (`Location` header) |
| Token | `POST /oauth2/token` | 200 | `{"access_token","token_type":"Bearer","expires_in":300}` |
| Userinfo | `GET /oauth2/userinfo` | 200 | `{"sub":"u-10001","name":"Alice","org_id":"org-10","org_name":"Example Org"}` |

All JSON responses use `Content-Type: application/json; charset=utf-8` with `Cache-Control: no-store` and `Pragma: no-cache`. The `access_token` is `at-` followed by 32 lowercase hex characters (35 chars total) and lives 300 seconds.

## Failure cases

- Unknown path → 404 `{"error":"not_found",...}`.
- Wrong method on a known path → 405 `{"error":"invalid_method",...}` with an `Allow` header injected by the host.
- Bad authorize parameters → 400 `invalid_request`; no redirect is issued.
- Signature missing/malformed/out-of-window/replayed → 401 `invalid_signature`.
- Bad `client_id`/`client_secret` → 401 `invalid_client`.
- Code invalid/expired/used, or `redirect_uri` mismatch → 400 `invalid_grant`.
- Missing or bad Bearer token → 401 `invalid_token`.
- Non-form body on token → 400 `invalid_request`.

No HTML error page is ever produced; every error body is JSON with only `error` and `error_description`.

## Cleanup

The demo holds no persistent state: codes, tokens, and signature request IDs live in process memory with TTLs (60 s code, 300 s token, 10 min request ID). Stop the server (`Ctrl-C`) and all in-memory state is cleared. No files are written and no external services are contacted.

## Scope and unsupported behavior

- **Authorization code grant only.** `response_type` must be `code`; `grant_type` must be `authorization_code`.
- **Not supported:** user login or real authentication, refresh tokens, PKCE, OIDC discovery, JWKS, ID tokens, logout/session management, dynamic client registration, multi-tenant isolation, persistence across restarts, or multi-instance coordination.
- The demo does not implement full OAuth 2.0 or OIDC compatibility; it is a test fixture for client integration behavior.

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
- `demo/scripts/userinfo.js` — Userinfo endpoint script
- `demo/scripts/http-errors.js` — Fallback route script for unmapped paths (404) and unsupported methods (405)
- `demo/stuntdouble.toml` — Route declarations for the demo fixture

## Tests

```bash
cargo test --test cli oauth_
```

Covers A01–A20 from the [contract](plans/oauth-authorization-mock.md §12): authorize parameter validation and state boundaries, token exchange (A05–A14), userinfo (A15–A18: Bearer validation, signature-before-token precedence, token expiry, exact response shape, replay rejection, canonical-query signing, log hygiene), and fallback responses (A19–A20: 404 `not_found`, 405 `invalid_method` with `Allow`).

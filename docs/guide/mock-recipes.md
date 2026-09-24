# Mock recipes

**English** \| [中文](./mock-recipes.zh-CN.md)

Task-oriented examples for common integration tests. Each recipe is additive: copy the route into `stuntdouble.toml`, create the referenced script, then validate and run. The exact key validation and `ctx` rules live in the [configuration contract](../contracts/config.md) and the [`ctx` API contract](../contracts/ctx-api.md); command behaviour lives in the [CLI contract](../contracts/cli.md).

For coding agents:

1. Read the configuration, `ctx` API, and CLI contracts before generating a mock.
2. Copy the closest recipe instead of inventing a new execution model.
3. Run `stuntdouble validate` before `serve`.
4. Verify behavior with `curl` and the request log.
5. Keep secrets and request payloads out of `ctx.log.*`; the host does not redact script-authored log messages.
6. Use `types/ctx-api-v1.d.ts` from the release for editor type checking.

## Dynamic JSON response

Mock a lookup endpoint that changes the answer by path parameter and query string.

```toml
[[routes]]
name = "get-user"
method = "GET"
path = "/users/:id"
script = "scripts/get-user.js"
```

```js
const user = {
  id: ctx.request.params.id,
  status: "active",
  includeEmail: ctx.request.query.include === "email",
};

if (user.includeEmail) {
  user.email = "user@example.com";
}

ctx.respond(
  200,
  { "Content-Type": "application/json; charset=utf-8" },
  JSON.stringify(user),
);
```

Run and check:

```bash
curl -i http://127.0.0.1:3000/users/42
curl -i 'http://127.0.0.1:3000/users/42?include=email'
```

## Validate and mock a POST

Return 201 for a valid JSON body and 400 for missing data. v1 keeps no cross-request state, so the response must be derived from the request.

```toml
[[routes]]
name = "create-ticket"
method = "POST"
path = "/tickets"
script = "scripts/create-ticket.js"
```

```js
let body = null;

try {
  body = JSON.parse(ctx.request.bodyText);
} catch (error) {
  body = null;
}

if (body && typeof body.title === "string" && body.title.length > 0) {
  ctx.respond(
    201,
    { "Content-Type": "application/json; charset=utf-8" },
    JSON.stringify({ id: "ticket-1", title: body.title, status: "open" }),
  );
} else {
  ctx.respond(
    400,
    { "Content-Type": "application/json; charset=utf-8" },
    JSON.stringify({ error: "invalid_ticket" }),
  );
}
```

Run and check:

```bash
curl -i http://127.0.0.1:3000/tickets \
  -H 'Content-Type: application/json' \
  --data '{"title":"payment failed"}'

curl -i http://127.0.0.1:3000/tickets \
  -H 'Content-Type: application/json' \
  --data '{}'
```

## Transform an upstream JSON response

Read an allowlisted upstream API and reshape its JSON before returning it.

```toml
[upstream]
allow_hosts = ["api.example.com"]

[[routes]]
name = "get-product"
method = "GET"
path = "/products/:id"
script = "scripts/get-product.js"
```

```js
const upstream = ctx.http.get("https://api.example.com/products/" + ctx.request.params.id);

if (upstream.status >= 400) {
  ctx.respond(
    502,
    { "Content-Type": "application/json; charset=utf-8" },
    JSON.stringify({ error: "product_bad_gateway" }),
  );
} else {
  const source = JSON.parse(upstream.text());

  ctx.respond(
    200,
    { "Content-Type": "application/json; charset=utf-8" },
    JSON.stringify({ id: source.id, name: source.display_name }),
  );
}
```

Run and check:

```bash
curl -i http://127.0.0.1:3000/products/42
```

Treat an upstream 4xx or 5xx status as data and choose your client-facing status explicitly. An uncaught transport failure answers `502 upstream_unreachable`.

## Serve a local file with Range support

Stream a file without loading it into the script heap. The host owns `Accept-Ranges`, `Content-Length`, and `Content-Range`; never set them yourself.

```toml
[[routes]]
name = "get-report"
method = "GET"
path = "/reports/:name"
script = "scripts/get-report.js"
```

```js
ctx.respond(
  200,
  { "Content-Type": "application/pdf" },
  ctx.file.stream("reports/" + ctx.request.params.name),
);
```

Run and check:

```bash
curl -i http://127.0.0.1:3000/reports/report.pdf
curl -i -H 'Range: bytes=0-1023' http://127.0.0.1:3000/reports/report.pdf
```

`ctx.file.stream` accepts only paths inside `files.root`.

## Return multipart upload metadata

Inspect the first uploaded file without persisting it.

```toml
[[routes]]
name = "upload-document"
method = "POST"
path = "/documents/upload"
script = "scripts/upload-document.js"
```

```js
const file = ctx.request.files[0];

if (!file) {
  ctx.respond(
    400,
    { "Content-Type": "application/json; charset=utf-8" },
    JSON.stringify({ error: "document_required" }),
  );
} else {
  ctx.respond(
    201,
    { "Content-Type": "application/json; charset=utf-8" },
    JSON.stringify({
      field: file.field,
      filename: file.filename,
      content_type: file.contentType,
      size: file.size,
    }),
  );
}
```

Run and check:

```bash
curl -i -F 'document=@./report.pdf;type=application/pdf' \
  http://127.0.0.1:3000/documents/upload
```

Upload bytes stay in request-scoped temporary storage and are removed after the request.

## Minimum CI verification

Validate first, start the server in the background, poll it, assert one response, then stop it:

```bash
stuntdouble validate --config stuntdouble.toml

stuntdouble serve --config stuntdouble.toml >server.log 2>&1 &
server_pid=$!
trap 'kill "$server_pid" 2>/dev/null || true; wait "$server_pid" 2>/dev/null || true' EXIT

for attempt in 1 2 3 4 5; do
  curl --silent --show-error --fail http://127.0.0.1:3000/users/42 && break
  sleep 1
done

curl --silent --show-error --fail --output response.json \
  http://127.0.0.1:3000/users/42
grep -q '"status":"active"' response.json
```

For failures, inspect `server.log`. With `--verbose`, 500/502 bodies include a stable `detail` class, but do not enable that flag in shared environments.

## Next steps

- [Getting started](./getting-started.md) — install and first-route workflow.
- [Demo fixture](../../demo/README.md) — a complete document manifest, download, and upload scenario.
- [Public contracts](../contracts/README.md) — authoritative configuration, `ctx` API, and CLI rules.

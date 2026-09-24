# Mock 场景示例

**English** \| [中文](./mock-recipes.zh-CN.md)

本页是英文版译本；以英文版为准。

这里是常见集成联调测试的任务型示例。每个示例都是增量配置：把 Route 复制到 `stuntdouble.toml`，创建引用的脚本，然后校验并运行。字段校验与 `ctx` 精确规则见[配置契约](../contracts/config.zh-CN.md)与[`ctx` API 契约](../contracts/ctx-api.zh-CN.md)。

给 coding agent 的检查清单：

1. 生成 mock 前先读配置契约、`ctx` API 契约与 CLI 契约。
2. 优先复制最接近的场景示例，不要发明第二套执行模型。
3. 先运行 `stuntdouble validate`，再运行 `serve`。
4. 使用 `curl` 和请求日志验证行为。
5. 不要把敏感信息或请求 payload 写入 `ctx.log.*`；宿主不会脱敏脚本日志。
6. 编辑器类型检查使用发布产物中的 `types/ctx-api-v1.d.ts`。

## 动态 JSON 响应

Mock 一个按路径参数和查询字符串返回不同结果的查询接口。

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

运行并验证：

```bash
curl -i http://127.0.0.1:3000/users/42
curl -i 'http://127.0.0.1:3000/users/42?include=email'
```

## 校验并 Mock POST

对合法 JSON 返回 201，对缺少数据返回 400。v1 没有请求间共享状态，因此响应只能从当前请求推导。

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

运行并验证：

```bash
curl -i http://127.0.0.1:3000/tickets \
  -H 'Content-Type: application/json' \
  --data '{"title":"payment failed"}'

curl -i http://127.0.0.1:3000/tickets \
  -H 'Content-Type: application/json' \
  --data '{}'
```

## 转换上游 JSON 响应

读取 allowlist 内的上游 API，先转换 JSON 再返回给客户端。

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

运行并验证：

```bash
curl -i http://127.0.0.1:3000/products/42
```

上游 4xx/5xx 是数据；请显式选择返回给客户端的状态码。未捕获的传输层失败返回 `502 upstream_unreachable`。

## 返回支持 Range 的本地文件

流式返回文件，不把文件读入脚本堆。宿主拥有 `Accept-Ranges`、`Content-Length` 和 `Content-Range`；不要在脚本中设置这些 header。

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

运行并验证：

```bash
curl -i http://127.0.0.1:3000/reports/report.pdf
curl -i -H 'Range: bytes=0-1023' http://127.0.0.1:3000/reports/report.pdf
```

`ctx.file.stream` 只接受 `files.root` 内的路径。

## 返回 multipart 上传元数据

读取第一个上传文件的元数据，不持久化文件内容。

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

运行并验证：

```bash
curl -i -F 'document=@./report.pdf;type=application/pdf' \
  http://127.0.0.1:3000/documents/upload
```

上传内容保存在请求级临时存储中，请求结束后清理。

## 最小 CI 验证

先校验配置，后台启动服务，轮询就绪，断言一个响应，然后停止进程：

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

失败时查看 `server.log`。`--verbose` 会让 500/502 响应包含稳定的 `detail` 类别，但不要在共享环境中开启。

## 下一步

- [快速开始](./getting-started.zh-CN.md) —— 安装与首个 Route。
- [演示夹具](../../demo/README.zh-CN.md) —— 完整的文档清单、下载与上传场景。
- [公开契约](../contracts/README.zh-CN.md) —— 配置、`ctx` API 与 CLI 的权威规则。

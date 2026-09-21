# Getting started

**English** \| [中文](./getting-started.zh-CN.md)

This page walks through the smallest useful Stunt Double setup: install the binary, describe one route, run the server, and check the result. The contract details live in [public contracts](../contracts/); this guide links to them instead of repeating them.

## Requirements

- A Rust toolchain. The crate builds on stable Rust; `Cargo.toml` declares the MSRV floor.

## Install

```bash
cargo install --path .          # installs the stuntdouble binary
# or run straight from the checkout
cargo run -- serve --config stuntdouble.toml
```

## Write a configuration

Create `stuntdouble.toml` next to your scripts:

```toml
config_version = "1"

[server]
bind = "127.0.0.1"
port = 3000

[files]
root = "./files"

[[routes]]
name = "hello"
method = "GET"
path = "/hello/:name"
script = "scripts/hello.js"
```

`files.root` must exist even when no route reads files yet. The [configuration contract](../contracts/config.md) lists every key and its validation rules.

## Write your first route

`scripts/hello.js`:

```js
const name = ctx.request.params.name;
ctx.respond(200, { "Content-Type": "text/plain; charset=utf-8" }, "hello " + name + "\n");
```

## Validate and run

```bash
stuntdouble validate --config stuntdouble.toml   # schema and file checks, no sockets
stuntdouble serve --config stuntdouble.toml
curl -i http://127.0.0.1:3000/hello/world
```

## Call an upstream API

Upstream calls only reach hosts listed in `[upstream] allow_hosts`:

```toml
[upstream]
allow_hosts = ["metadata.example.com"]
timeout_ms = 15000
```

```js
const upstream = ctx.http.get("https://metadata.example.com/documents");
if (upstream.status >= 400) {
  ctx.respond(502, { "Content-Type": "application/json" }, '{"error":"metadata_bad_gateway"}');
} else {
  ctx.respond(200, { "Content-Type": "application/json" }, upstream.text());
}
```

4xx/5xx answers are data; only transport failures throw `upstream_unreachable`. For a large or binary body, use `ctx.http.pipe`, which streams the upstream body straight to the client and preserves `Range`/206 — see the [demo fixture](../../demo/README.md) and the [`ctx` API contract](../contracts/ctx-api.md).

## When something fails

| You see | Meaning |
| --- | --- |
| 404 `not_found` | no route matched the method and path |
| 500 `script_error` | the script threw, timed out, or failed to load; the stack stays in the server log |
| 500 `script_no_response` | the script finished without calling `ctx.respond` |
| 502 `upstream_unreachable` | an uncaught transport failure (DNS, connection, TLS, or timeout) |
| validation error with a dotted path | the configuration violates the contract; the message names the field and the expected shape |

`sandbox.script_timeout_ms` bounds script runtime; the default is 10000 ms.

Add `--verbose` to `serve` when diagnosing one of these failures:

```bash
stuntdouble serve --config stuntdouble.toml --verbose
```

The 500/502 JSON body then carries a stable `detail` string such as `upstream transport failure: timeout`; it never includes stack traces, script messages, upstream bodies, or internal addresses. Treat it as local diagnostic output and leave it off in shared environments. `serve` also writes one structured JSON log line per request to stderr, including the upstream call chain, body sizes, and allowlisted headers. A streamed response writes its line when the body ends; a mid-stream upstream failure appears as `upstream_stream_error`.

## Next steps

- [Public contracts](../contracts/) — configuration, `ctx` API, and CLI.
- [Demo fixture](../../demo/README.md) — manifest CSV generation plus PDF streaming with Range.
- [Product definition](../../plans/product-definition.md) (Chinese) — v1 scope and non-goals.

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

Release builds also target Linux x86_64, macOS arm64, and Windows x86_64 and publish a container image. The release contract requires archives, checksums, attestations, an SBOM, the `ctx` API type definition, and a GHCR tag plus digest for each release; download and verification commands are in the [README's Install and verify section](../../README.md#install-and-verify).

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

`files.root` must exist; it is the only directory `ctx.file` can read. The [configuration contract](../contracts/config.md) lists every key and its validation rules.

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

## Stop the server

Press Ctrl-C (SIGINT) or send SIGTERM on Unix. The first signal stops accepting new connections, drains in-flight requests, and exits `0`.

If the shutdown takes too long, send the signal again: a second signal observed after the graceful shutdown has started abandons the drain and exits immediately with `130` (SIGINT/Ctrl-C) or `143` (SIGTERM). Standard signals are not queued, so two signals sent back-to-back before the first is observed may be coalesced; that case still drains normally and exits `0`. See the [CLI contract](../contracts/cli.md) for the full rules.

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

## Read and stream local files

`ctx.file` reads only inside `files.root`. Absolute paths and any `..` component are rejected, symlinks must resolve inside the root, and buffered reads are capped at 8 MiB:

```js
const text = ctx.file.readText("metadata.json");
ctx.respond(200, { "Content-Type": "application/json" }, text);
```

`readText` decodes strict UTF-8; `readBytes` returns a `Uint8Array`. Both throw catchable errors (`file_path_invalid`, `file_not_found`, `file_too_large`, `file_encoding_error`, `file_io_error`), and an uncaught one answers 500 `script_error`.

To serve a file without loading it into the script heap, pass `ctx.file.stream(path)` as the `ctx.respond` body with status `200`. The host owns `Accept-Ranges`, `Content-Length`, and `Content-Range`: a valid single `Range` answers 206, an unusable one answers 416, and `If-Range` disables range handling. `Content-Type` is never inferred, so set it in the response headers.

```js
ctx.respond(200, { "Content-Type": "application/pdf" }, ctx.file.stream("documents/DOC-0001.pdf"));
```

## Handle multipart uploads

A matched `multipart/form-data` request is parsed by the host before the route script runs. `[files] upload_max_bytes` caps file and non-file field data (default `20971520`), while the whole multipart body, including framing, has a 1 MiB framing allowance. A malformed part or a part without a `name` attribute answers 400 `invalid_multipart`; data over the limit answers 413 `upload_too_large`. Both use the project JSON error envelope with `request_id` and never run the script.

```js
const file = ctx.request.files[0];
if (!file) {
  ctx.respond(400, { "Content-Type": "application/json" }, '{"error":"document_required"}');
} else {
  ctx.respond(200, { "Content-Type": "text/plain; charset=utf-8" }, file.text());
}
```

`ctx.request.files` is a read-only array of `{field, filename, contentType, size, text(), bytes(), stream()}`. `filename` is the client basename after `/` and `\` path components are stripped; no temporary path is exposed. Non-multipart requests expose `[]`; non-file fields are ignored but still count toward the limit.

`text()` decodes strict UTF-8, and `text()`/`bytes()` are capped at 8 MiB. `stream()` is uncapped, opaque, single-consumption, and valid only as the `ctx.respond` body in the same request; it follows the same Range and host-owned framing rules as `ctx.file.stream`. Uploads use request-scoped temporary storage and are removed when the request ends or the stream finishes — see the [`ctx` API contract](../contracts/ctx-api.md) and [configuration contract](../contracts/config.md) for exact shapes.

## When something fails

| You see | Meaning |
| --- | --- |
| 404 `not_found` | no route matched the method and path |
| 500 `script_error` | the script threw, timed out, or failed to load; the stack stays in the server log |
| 500 `script_no_response` | the script finished without calling `ctx.respond` |
| 502 `upstream_unreachable` | an uncaught transport failure (DNS, connection, TLS, or timeout) |
| validation error with a dotted path | the configuration violates the contract; the message names the field and the expected shape |

`sandbox.script_timeout_ms` bounds script runtime; the default is 10000 ms. A script also runs under a loop-iteration backstop and recursion/VM-stack limits, and an engine panic answers 500 `script_error`. Boa 0.22 exposes no heap metric or interrupt hook, so no in-process heap cap is enforced — see [SECURITY.md](../../SECURITY.md) for the threat model and the [ADR 0003](../../plans/adr/0003-script-first-multi-runtime.md) T3 amendment for the tradeoff.

Add `--verbose` to `serve` when diagnosing one of these failures:

```bash
stuntdouble serve --config stuntdouble.toml --verbose
```

The 500/502 JSON body then carries a stable `detail` string such as `upstream transport failure: timeout`; it never includes stack traces, script messages, upstream bodies, or internal addresses. Treat it as local diagnostic output and leave it off in shared environments. `serve` also writes one structured JSON log line per request to stderr, including the upstream call chain, body sizes, and allowlisted headers. A streamed response writes its line when the body ends; a mid-stream failure appears as `upstream_stream_error` or `file_stream_error`, and a client that leaves mid-body is recorded as `client_disconnected`.

## Next steps

- [Public contracts](../contracts/) — configuration, `ctx` API, and CLI.
- [Demo fixture](../../demo/README.md) — manifest CSV generation plus PDF streaming with Range.
- [Product definition](../../plans/product-definition.md) (Chinese) — v1 scope and non-goals.

# Document manifest and download demo

Runnable Stunt Double fixture for the document contracts in
[plans/demo-document-catalog.md](../plans/demo-document-catalog.md) §3. The
route `GET /demo/documents/manifest/:group` reads upstream metadata and answers
`text/plain; charset=utf-8` CSV; `GET /demo/documents/download/:document_id`
matches the metadata by `code` and streams the referenced PDF.

Everything here uses the public demo names from the catalog; it holds no real
data.

## Run it

```bash
METADATA_API_URL=https://metadata.example.com/demo/documents \
  stuntdouble serve --config demo/stuntdouble.toml
```

`METADATA_API_URL` is optional and defaults to the same URL. `allow_hosts` in
`stuntdouble.toml` lists the only hosts the scripts can reach: the metadata API
and the demo PDF host.

```bash
curl -i http://127.0.0.1:3000/demo/documents/manifest/group-a
curl -fSLo DOC-0001.pdf http://127.0.0.1:3000/demo/documents/download/DOC-0001
curl -i -H 'Range: bytes=0-1023' \
  http://127.0.0.1:3000/demo/documents/download/DOC-0001
```

Files:

- `stuntdouble.toml` — route declarations plus the upstream allowlist.
- `scripts/manifest.js` — CSV transform.
- `scripts/download.js` — document lookup plus PDF passthrough.
- `files/` — the static file root the configuration contract requires; file
  capabilities arrive in a later slice.

## Manifest contract

- Header row `文件编码,文件标题,系统代码`; data rows follow
  `code,title,system_code`, and `pdf_url` never appears in the manifest.
- A field containing a comma, a double quote, CR, or LF is wrapped in double
  quotes, and its internal double quotes are doubled.
- An empty `data` array answers 200 with the header row and one trailing newline.
- `:group` is kept for existing callers and does not filter the metadata.
- Metadata non-2xx responses, unreadable JSON, a missing `data` array, and
  transport failures answer 502 `{"error":"metadata_bad_gateway"}`. Allowlist
  and URL policy failures stay 500 `script_error`, per
  [ADR 0005](../plans/adr/0005-upstream-failure-semantics.md).
- The client `X-Request-ID` is never forwarded upstream; the server mints its
  own request id for every response.

## Download contract

- `:document_id` matches a metadata entry whose `code` equals it exactly; an
  unknown id answers 404 `{"error":"document_not_found"}` without contacting
  any PDF URL.
- A missing, unparsable, or non-`http`/`https` `pdf_url` answers
  502 `{"error":"pdf_url_invalid"}`; the host reports the URL rejection to the
  script as `upstream_url_invalid`.
- The PDF body streams to the client through `ctx.http.pipe`, so the bytes never
  enter the script heap. The response carries `Content-Type: application/pdf`
  and `Content-Disposition: attachment;filename="DOC-0001.pdf"`.
- A client `Range` header is forwarded to the upstream call, and an upstream 206
  response reaches the client with its `Content-Range` header.
- The body streams as it arrives: an upstream `Content-Length` is passed
  through, while a chunked upstream response reaches the client chunked and
  without one. Once the response head is on the wire, a mid-body upstream
  failure can only truncate the body.
- PDF transport failures, non-2xx PDF answers, and redirect chains the host
  cannot follow (more than 3 hops, an unusable `Location`, or a 3xx answer
  without one) all answer 502 `{"error":"pdf_bad_gateway"}`; metadata failures
  answer 502 `metadata_bad_gateway`. Allowlist rejections stay 500
  `script_error`, per the T6 amendment to
  [ADR 0005](../plans/adr/0005-upstream-failure-semantics.md).

## Test

The end-to-end checks serve this fixture with a stdlib fake upstream:

```bash
cargo test --test cli demo_
```

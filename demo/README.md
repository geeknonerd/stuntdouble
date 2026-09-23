# Document manifest and download demo

**English** \| [中文](./README.zh-CN.md)

Runnable Stunt Double fixture for the document contracts in
[plans/demo-document-catalog.md](../plans/demo-document-catalog.md) §3.

- `GET /demo/documents/manifest/:group` reads upstream metadata and answers
  `text/plain; charset=utf-8` CSV; `GET /demo/documents/download/:document_id`
  matches the metadata by `code` and streams the referenced PDF through
  `ctx.http.pipe`.
- The offline file slice needs no upstream and no environment override:
  `GET /demo/documents/local-manifest/:group` and
  `GET /demo/documents/local-download/:document_id` read and stream the fixtures
  under `files/`, while `POST /demo/documents/upload` answers request-scoped
  multipart metadata and persists nothing.

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

The offline routes need no `METADATA_API_URL`, no `allow_hosts` entry, and no
network:

```bash
stuntdouble serve --config demo/stuntdouble.toml

curl -i http://127.0.0.1:3000/demo/documents/local-manifest/group-a
curl -fSLo DOC-0001.pdf \
  http://127.0.0.1:3000/demo/documents/local-download/DOC-0001
curl -i -H 'Range: bytes=0-1023' \
  http://127.0.0.1:3000/demo/documents/local-download/DOC-0001
curl -i -F document=@DOC-0001.pdf \
  http://127.0.0.1:3000/demo/documents/upload
```

Add `--verbose` to `serve` when diagnosing an engine-generated 500/502: the
JSON body then carries a stable `detail` field. The demo's own 502 bodies
(`metadata_bad_gateway`, `pdf_url_invalid`, `pdf_bad_gateway`) are
script-authored mappings, so the host does not inject `detail` into them.

Files:

- `stuntdouble.toml` — route declarations plus the upstream allowlist.
- `scripts/manifest.js` — upstream CSV transform.
- `scripts/download.js` — upstream document lookup plus PDF passthrough.
- `scripts/local-manifest.js` — CSV transform over `files/metadata.json`.
- `scripts/local-download.js` — fixture document lookup plus local PDF stream.
- `scripts/upload.js` — multipart upload echo.
- `files/metadata.json` — public document fixture rows (`code`, `title`,
  `system_code`, and the root-relative `file`).
- `files/DOC-0001.pdf`, `files/DOC-0002.pdf` — minimal public PDF fixtures.
- `files/` is the only static file root; uploads use separate request-scoped
  temporary storage and never write here.

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
  failure can only truncate the body; the host records it in the request log
  as `upstream_stream_error` when the stream ends.
- PDF transport failures, non-2xx PDF answers, and redirect chains the host
  cannot follow (more than 3 hops, an unusable `Location`, or a 3xx answer
  without one) all answer 502 `{"error":"pdf_bad_gateway"}`; metadata failures
  answer 502 `metadata_bad_gateway`. Allowlist rejections stay 500
  `script_error`, per the T6 amendment to
  [ADR 0005](../plans/adr/0005-upstream-failure-semantics.md).

## Local file contracts

### Local manifest

- `GET /demo/documents/local-manifest/:group` answers the manifest contract
  above from `files/metadata.json` instead of an upstream call: the same header
  row, the same `code,title,system_code` order, the same quoting rule, and the
  same header-only body for an empty `data` array. The root-relative `file`
  field never appears in the CSV.
- The fixture is re-read on every request with `ctx.file.readText`: v1 keeps no
  cross-request state, so an edited fixture is visible to the next request
  without a restart.
- A missing, unreadable, non-UTF-8, or malformed fixture is a broken demo rather
  than a client error. The uncaught file or JSON error stays the engine default
  500 `script_error`; `ctx.file` never maps a missing file to 404.

### Local download

- `GET /demo/documents/local-download/:document_id` matches `code` in
  `files/metadata.json` exactly and streams the entry's root-relative `file`
  with `ctx.file.stream`; no upstream is contacted.
- A full request answers 200 with `Content-Type: application/pdf`,
  `Content-Disposition: attachment;filename="DOC-0001.pdf"`, and the host-owned
  `Accept-Ranges` and `Content-Length`.
- The host owns range framing: a valid single `Range` answers 206 with
  `Content-Range`, an unsatisfiable, malformed, or multi-range request answers
  416 with `Content-Range: bytes */<size>` and no body, and a request carrying
  `If-Range` answers the full 200.
- An unknown id answers 404 `{"error":"document_not_found"}` without opening a
  file. A `file` that is missing, absolute, contains `..`, or escapes the root
  stays a catchable script error and answers 500 `script_error`.

### Upload

- `POST /demo/documents/upload` is parsed by the host before the script runs;
  `ctx.request.files` exposes the file parts in multipart order, and non-file
  form fields are ignored.
- The route answers 201 with the first file part's metadata as JSON:
  `{"field":"document","filename":"DOC-0001.pdf","content_type":"application/pdf","size":596}`.
  `filename` is the client-provided basename, and `content_type` is `null` when
  the part carries no `Content-Type`.
- Nothing is persisted: the bytes stay in request-scoped temporary storage that
  is removed when the request ends, the static file root is never written, and a
  later request cannot observe the upload.
- Multipart failures happen before the script: malformed multipart answers 400
  `invalid_multipart`, and data over `files.upload_max_bytes` answers 413
  `upload_too_large`. See the
  [configuration contract](../docs/contracts/config.md).

## Test

`cargo test --test cli demo_` serves this fixture and covers both route
families: the upstream routes run against a stdlib fake upstream, while the
offline routes run with no upstream and no environment override — including CSV
escaping and the header-only empty-data case, 200/206/416/404 on the local
download, and the upload response with its no-persistence assertion.

```bash
cargo test --test cli demo_
```

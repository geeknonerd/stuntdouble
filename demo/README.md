# Document manifest demo

Runnable Stunt Double fixture for the CSV manifest contract in
[plans/demo-document-catalog.md](../plans/demo-document-catalog.md) §3.1. The
route `GET /demo/documents/manifest/:group` reads upstream metadata and answers
`text/plain; charset=utf-8` CSV.

Everything here uses the public demo names from the catalog; it holds no real
data.

## Run it

```bash
METADATA_API_URL=https://metadata.example.com/demo/documents \
  stuntdouble serve --config demo/stuntdouble.toml
```

`METADATA_API_URL` is optional and defaults to the same URL. `allow_hosts` in
`stuntdouble.toml` is the only host the script can reach.

```bash
curl -i http://127.0.0.1:3000/demo/documents/manifest/group-a
```

Files:

- `stuntdouble.toml` — route declaration plus the metadata allowlist.
- `scripts/manifest.js` — CSV transform.
- `files/` — the static file root the configuration contract requires; file
  capabilities arrive in a later slice.

## Contract

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

## Test

The end-to-end checks serve this fixture with a stdlib fake upstream:

```bash
cargo test --test cli demo_
```

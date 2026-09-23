# 文档清单与下载演示

[English](./README.md) \| **中文**

> 本页是英文版 [README.md](./README.md) 的译本；如有出入，以英文版为准。

这是 [plans/demo-document-catalog.md](../plans/demo-document-catalog.md) §3 中文档契约的可运行 Stunt Double 夹具。

- `GET /demo/documents/manifest/:group` 读取上游元数据并答 `text/plain; charset=utf-8` CSV；`GET /demo/documents/download/:document_id` 按 `code` 匹配元数据，并经 `ctx.http.pipe` 流式转发被引用的 PDF。
- 离线文件切片不需要上游，也不需要环境变量覆盖：`GET /demo/documents/local-manifest/:group` 与 `GET /demo/documents/local-download/:document_id` 读取并流式返回 `files/` 下的夹具，`POST /demo/documents/upload` 则回显请求级 multipart 元数据，且不持久化任何内容。

这里的一切都使用清单中的公开演示名称，不含任何真实数据。

## 运行

```bash
METADATA_API_URL=https://metadata.example.com/demo/documents \
  stuntdouble serve --config demo/stuntdouble.toml
```

`METADATA_API_URL` 可选，默认就是同一 URL。`stuntdouble.toml` 中的 `allow_hosts` 列出脚本唯一能访问的 host：元数据 API 与演示 PDF host。

```bash
curl -i http://127.0.0.1:3000/demo/documents/manifest/group-a
curl -fSLo DOC-0001.pdf http://127.0.0.1:3000/demo/documents/download/DOC-0001
curl -i -H 'Range: bytes=0-1023' \
  http://127.0.0.1:3000/demo/documents/download/DOC-0001
```

离线路由不需要 `METADATA_API_URL`、不需要 `allow_hosts` 条目，也不访问网络：

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

诊断引擎生成的 500/502 时，给 `serve` 加 `--verbose`：JSON body 会带稳定的 `detail` 字段。演示自身的 502 body（`metadata_bad_gateway`、`pdf_url_invalid`、`pdf_bad_gateway`）是脚本映射的结果，宿主不会向其中注入 `detail`。

文件：

- `stuntdouble.toml` —— 路由声明与上游 allowlist。
- `scripts/manifest.js` —— 上游 CSV 变换。
- `scripts/download.js` —— 上游文档查找与 PDF 透传。
- `scripts/local-manifest.js` —— 读取 `files/metadata.json` 的 CSV 变换。
- `scripts/local-download.js` —— 夹具文档查找与本地 PDF 流。
- `scripts/upload.js` —— multipart 上传回显。
- `files/metadata.json` —— 公开文档夹具记录（`code`、`title`、`system_code` 与根内相对路径 `file`）。
- `files/DOC-0001.pdf`、`files/DOC-0002.pdf` —— 最小公开 PDF 夹具。
- `files/` 是唯一静态文件根；上传使用独立的请求级临时存储，绝不写入这里。

## 清单契约

- 表头行 `文件编码,文件标题,系统代码`；数据行按 `code,title,system_code` 顺序，`pdf_url` 绝不出现在清单中。
- 含逗号、双引号、CR 或 LF 的字段用双引号包裹，内部双引号翻倍。
- `data` 为空数组时答 200，只有表头行加一个结尾换行。
- `:group` 为既有调用方保留，不过滤元数据。
- 元数据非 2xx、JSON 不可读、缺少 `data` 数组以及传输层失败都答 502 `{"error":"metadata_bad_gateway"}`。allowlist 与 URL 策略失败保持 500 `script_error`，见 [ADR 0005](../plans/adr/0005-upstream-failure-semantics.md)。
- 客户端的 `X-Request-ID` 绝不转发给上游；服务端为每个响应生成自己的 request id。

## 下载契约

- `:document_id` 精确匹配某条元数据的 `code`；未知 id 答 404 `{"error":"document_not_found"}`，且不访问任何 PDF URL。
- `pdf_url` 缺失、无法解析或非 `http`/`https` 时答 502 `{"error":"pdf_url_invalid"}`；宿主把该 URL 拒绝以 `upstream_url_invalid` 报告给脚本。
- PDF body 经 `ctx.http.pipe` 流给客户端，因此字节从不进入脚本堆。响应带 `Content-Type: application/pdf` 与 `Content-Disposition: attachment;filename="DOC-0001.pdf"`。
- 客户端 `Range` header 转发给上游调用，上游 206 响应连同其 `Content-Range` header 抵达客户端。
- body 边到边流：上游 `Content-Length` 会被透传，而 chunked 上游响应以 chunked、无长度抵达客户端。响应头一旦上线，body 中途的上游失败只能截断 body；宿主在流结束时把该失败记为请求日志中的 `upstream_stream_error`。
- PDF 传输失败、PDF 非 2xx 响应，以及宿主无法跟随的重定向链（超过 3 跳、`Location` 不可用，或 3xx 且没有 `Location`）都答 502 `{"error":"pdf_bad_gateway"}`；元数据失败答 502 `metadata_bad_gateway`。allowlist 拒绝保持 500 `script_error`，见 [ADR 0005](../plans/adr/0005-upstream-failure-semantics.md) 的 T6 修订。

## 本地文件契约

### 本地清单

- `GET /demo/documents/local-manifest/:group` 用 `files/metadata.json` 复现上面的清单契约，不访问上游：相同的表头行、相同的 `code,title,system_code` 顺序、相同的引号规则，以及 `data` 为空数组时只有表头行加一个结尾换行。根内相对路径 `file` 绝不出现在 CSV 中。
- 夹具由 `ctx.file.readText` 在每次请求时重新读取：v1 不提供请求间共享状态，因此修改夹具后下一次请求即可见，无需重启。
- 夹具缺失、不可读、非 UTF-8 或结构非法属于夹具损坏，而不是客户端错误：未捕获的文件/JSON 异常保持引擎默认的 500 `script_error`；`ctx.file` 绝不会把文件缺失映射成 404。

### 本地下载

- `GET /demo/documents/local-download/:document_id` 用 `files/metadata.json` 的 `code` 精确匹配，并把该记录的根内相对路径 `file` 经 `ctx.file.stream` 流给客户端；全程不联系上游。
- 完整请求答 200，带 `Content-Type: application/pdf`、`Content-Disposition: attachment;filename="DOC-0001.pdf"`，以及宿主的 `Accept-Ranges` 与 `Content-Length`。
- Range framing 归宿主所有：合法单区间 `Range` 答 206 并带 `Content-Range`；不可满足、格式非法或多区间的请求答 416，带 `Content-Range: bytes */<size>` 且无 body；带 `If-Range` 的请求答完整 200。
- 未知 id 答 404 `{"error":"document_not_found"}`，且不打开任何文件。`file` 缺失、为绝对路径、含 `..` 或逃出静态文件根时保持可捕获脚本错误，答 500 `script_error`。

### 上传

- `POST /demo/documents/upload` 由宿主在脚本运行前解析；`ctx.request.files` 按 multipart 顺序暴露文件 part，非文件表单字段被忽略。
- 路由用 201 回显第一个文件 part 的元数据：`{"field":"document","filename":"DOC-0001.pdf","content_type":"application/pdf","size":596}`。`filename` 是客户端提供的 basename；part 不带 `Content-Type` 时 `content_type` 为 `null`。
- 不持久化：内容只存在于请求结束时清理的请求级临时存储里，静态文件根绝不写入，后续请求无法观察到本次上传。
- multipart 层面的失败发生在脚本之前：结构非法答 400 `invalid_multipart`，数据超过 `files.upload_max_bytes` 答 413 `upload_too_large`。见[配置契约](../docs/contracts/config.zh-CN.md)。

## 测试

`cargo test --test cli demo_` 会启动该夹具并覆盖两类路由：上游路由针对标准库 fake upstream 运行；离线路由不启动上游、也不设置任何环境变量覆盖——覆盖 CSV 转义与空数据只有表头的情况、本地下载的 200/206/416/404，以及上传响应的「不持久化」断言。

```bash
cargo test --test cli demo_
```

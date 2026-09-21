# 文档清单与下载演示

[English](./README.md) \| **中文**

> 本页是英文版 [README.md](./README.md) 的译本；如有出入，以英文版为准。

这是 [plans/demo-document-catalog.md](../plans/demo-document-catalog.md) §3 中文档契约的可运行 Stunt Double 夹具。路由 `GET /demo/documents/manifest/:group` 读取上游元数据并答 `text/plain; charset=utf-8` CSV；`GET /demo/documents/download/:document_id` 按 `code` 匹配元数据，并流式转发被引用的 PDF。

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

诊断引擎生成的 500/502 时，给 `serve` 加 `--verbose`：JSON body 会带稳定的 `detail` 字段。演示自身的 502 body（`metadata_bad_gateway`、`pdf_url_invalid`、`pdf_bad_gateway`）是脚本映射的结果，宿主不会向其中注入 `detail`。

文件：

- `stuntdouble.toml` —— 路由声明与上游 allowlist。
- `scripts/manifest.js` —— CSV 变换。
- `scripts/download.js` —— 文档查找与 PDF 透传。
- `files/` —— 配置契约要求的静态文件根；文件能力在后续切片落地。

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

## 测试

端到端检查用标准库 fake upstream 驱动该夹具：

```bash
cargo test --test cli demo_
```

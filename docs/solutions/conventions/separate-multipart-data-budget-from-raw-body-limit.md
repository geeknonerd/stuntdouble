---
title: "Separate multipart data budgets from raw request body limits"
date: 2026-09-23
last_updated: 2026-09-24
category: conventions
module: multipart upload request limits
problem_type: convention
component: server
severity: medium
applies_when:
  - "在应用层 payload 预算之外再叠加框架层的原始请求体上限时"
  - "解析 multipart 等带 framing 的请求体，part 内容字节与 wire 字节不是同一个计数对象时"
  - "同一 Router/handler 同时服务 multipart 与非 multipart，却要保留不同上限与不同错误形状时"
  - "评审固定 framing allowance 是否会造成合法请求误拒或留下资源耗尽缺口时"
related_components: [config, files, tests]
symptoms:
  - "只按已解析的 part 内容计数时，超长 part name、header 或 boundary 等 framing 在数据预算生效前不受限"
  - "把原始流上限设成与数据预算相等的值时，接近预算的合法上传会在解析前被框架层拒绝"
  - "multipart 放宽 Router 级默认上限后，非 multipart 既有的 2 MiB 边界可能被静默抬高"
root_cause: logic_error
resolution_type: code_fix
tags: [multipart-upload, request-body-limits, framing-allowance, default-body-limit, axum, upload-max-bytes, non-multipart, dos-boundary]
---

# multipart 上传：数据预算与原始流上限必须分层

## Context

T12（issue #53）为命中 `multipart/form-data` 的路由加入请求体解析：宿主在脚本运行前把文件 part 流式写入请求级临时目录，非文件字段只读取计数、不落盘，文件元数据随后暴露为 `ctx.request.files`。这里表面上是“限制上传大小”，实际同时存在两个计数对象：

- 数据预算 `files.upload_max_bytes`：只计 part 的 content bytes，文件与非文件字段都计，不含 multipart framing；默认 20 MiB（`src/config.rs:44-47`、`src/config.rs:690-692`、`src/files.rs:316-320`）。
- 原始请求流上限：计整个 multipart body stream 的 wire bytes，包含 boundary、part header、CRLF 等 framing；取值是 `files.upload_max_bytes` 加一个固定的 1 MiB allowance（`src/server.rs:363-376`、`docs/contracts/config.md:80`、`SECURITY.md:23`）。

两个数字如果写成同一个，会得到两种相反的坏结果：上限偏紧时，接近数据预算的合法上传会在解析前被框架层拒绝；上限偏松或根本不存在时，攻击者可以用极长的 part name、header 或 boundary framing 消耗资源，而 part content 始终低于数据预算。

(session history) T12 的首版实现正是第二种形状：multer 的 whole-stream limit 保持默认（不受限），只在 `field.chunk()` 累计 part 内容，并且仅对带 `Content-Length` 的请求做固定 1 MiB framing 预检。两轮 code review 都指向同一缺口——无 `Content-Length` 的 chunked 上传在数据预算生效前没有任何原始流上限。修复把整体闸门提升到 axum 的 `DefaultBodyLimit`（数据预算 + 1 MiB allowance），并让 Content-Length 预检与整闸共用同一条公式。评审也质疑过 1 MiB allowance 没有被 spec 明确授权、可能误拒 framing 特别大的合法请求；复核后的决定是保留固定 allowance，因为无界 framing 是明确的资源耗尽面，且该上限已进入配置、CLI 与安全契约并配有回归测试。

T12 的 multipart 上传、收紧后的校验与账目已随 PR #57 合并进 `main`（issue #53 已关闭）；从 T12 拆出的读取期限 issue #56 由 `server.request_timeout_ms` 实现并随 PR #59 合并（issue #56 已关闭），它叠加在本文的字节分层之上。本文描述的是 `main` 上生效的既成状态。

相邻学习 `docs/solutions/conventions/host-boundary-fail-closed-input-classification.md` 曾预测 T12 会碰到“缺失、畸形、超限不能被压平成同一种失败”的同类形状。本文只记录其中的大小上限分层：错误形状（400 与 413 的区分、Content-Length 缺失、multipart 结构畸形）仍由那篇 convention 负责。

## Guidance

### 1. 把“数据预算”和“原始流闸门”当成两本账

数据预算的权威定义是 `files.upload_max_bytes`。它要求正整数；0、负数和非整数在加载配置时就会被拒绝（`src/config.rs:566-587`、`docs/contracts/config.md:30`）。解析器对每一个 `field.chunk()` 累加 `total_bytes`，一旦超过预算立即返回 `ParseFailure::TooLarge`；非文件字段同样累加，只是不落盘（`src/files.rs:356-373`）。解析结束后的 `UploadStore.total_bytes` 也是这份账目（`src/files.rs:397-400`）。

原始流闸门是另一本账。服务启动时计算：

```rust
let upload_body_limit = usize::try_from(
    config
        .files
        .upload_max_bytes
        .saturating_add(MULTIPART_FRAMING_ALLOWANCE),
)
.unwrap_or(usize::MAX);

Router::new()
    .fallback(any(handle))
    .layer(DefaultBodyLimit::max(upload_body_limit))
```

当前实现见 `src/server.rs:363-376`；`MULTIPART_FRAMING_ALLOWANCE` 是固定的 1 MiB 常量（`src/server.rs:671`）。公开契约把它描述为“整个 multipart body stream，including framing，受 `upload_max_bytes` 加 1 MiB 约束”，数据预算本身仍不含 framing（`docs/contracts/config.md:80`、`SECURITY.md:23`）。

因此有效的模型是：**数据预算决定“客户端可以提交多少内容”，原始流上限决定“框架在开始计数内容之前，允许 wire 上有多少字节”。** 后者必须严格大于前者，或者至少额外覆盖合法 framing；否则两者数值相等时，任何非空的 multipart framing 都会吃掉数据预算，配置值就不再是“可用的 part 内容字节数”。

### 2. 预检和框架闸门按 wire bytes，解析器按 payload bytes

命中路由后的 multipart 预检使用的是原始流公式，而不是数据预算：

```rust
if content_length
    .is_some_and(|len| len > max_bytes.saturating_add(MULTIPART_FRAMING_ALLOWANCE))
{
    return Err(PrepareError::Upload {
        failure: files::ParseFailure::too_large(0, None),
        request_body_bytes: content_length,
    });
}
match files::parse_multipart(request, max_bytes).await {
    // ...
}
```

当前实现见 `src/server.rs:172-186`。这里有两个必须同时成立的事实：

- `Content-Length` 预检比较的是 `max_bytes + MULTIPART_FRAMING_ALLOWANCE`，因为它判断的是 wire bytes；声明长度超过 raw ceiling 时，可以在读取 body 之前直接返回 413（`src/server.rs:172-186`）。
- `parse_multipart` 收到的仍然只有 `max_bytes`，所以流式账目不会被 1 MiB allowance 污染（`src/server.rs:163`、`src/files.rs:321-324`）。

没有 `Content-Length` 或客户端声明值不可信时，流式计数仍然有效；`multipart_limit_accepts_the_cap_and_rejects_precheck_chunked_and_form_fields` 同时覆盖了预算命中、超限、无 Content-Length 的 chunked 请求、非文件字段和声明长度预检（`tests/cli.rs:1901-1988`）。框架层 raw stream 被超过时，axum 把 multipart 解析失败映射为 413，本地分类代码据此返回同一个稳定类别 `upload_too_large`（`src/files.rs:421-430`、`src/files.rs:267-274`、`src/server.rs:116-133`）。这意味着“内容超预算”和“framing 把原始流顶穿”在客户端上共享 413 类别，但触发它们的账目不同，日志里的 `upload.total_bytes` 记录的仍是已读内容字节（`src/server.rs:186-203`）。

### 3. 共享 Router 默认值覆盖 multipart 原始流，应用层再按路径收紧

当前 Router 只有一个 `fallback(any(handle))`，业务路由匹配和 multipart/非 multipart 分支都在 handler 内完成（`src/server.rs:373-377`、`src/server.rs:439`）。因此 Router 层只能有一个共享的默认请求体值，不能因为“这条路由是 multipart、那条不是”自动切换两个上限。

这个共享值不能固定为 2 MiB：默认 `upload_max_bytes` 是 20 MiB，multipart 数据预算大于 2 MiB 时，合法的 multipart 会在 handler 之前被拒，配置项形同失效。它也不能只写成 `upload_max_bytes`：接近数据预算的合法 multipart 仍有 boundary、header、CRLF 开销，会在内容账目有机会判断之前被框架拒绝。当前实现因此把共享默认值设为 multipart 的原始流上限：`upload_max_bytes + 1 MiB`。

非 multipart 路径不依赖这个共享默认值，而是在 `prepare_request` 内显式使用 2 MiB：

```rust
match axum::body::to_bytes(request.into_body(), NON_MULTIPART_BODY_LIMIT).await {
    Ok(bytes) => { /* 请求快照 */ }
    Err(_) => Err(PrepareError::BodyTooLarge {
        request_body_bytes: content_length,
    }),
}
```

当前实现见 `src/server.rs:210-211`，`NON_MULTIPART_BODY_LIMIT` 是 `2 * 1024 * 1024`（`src/server.rs:668`）。这就是“应用层重新收紧”：在默认 20 MiB 配置下，框架共享值（21 MiB）比非 multipart 的 2 MiB 更宽，其语义是 multipart 的 raw ceiling；真正走非 multipart 分支时，读取上限重新变成 2 MiB。

一条可复用的规则是：**如果同一个共享层级真的同时承载两类请求，取较宽的那个上限，然后在能识别请求类别的最近位置按类别重新收紧。** 当前代码通过显式 `to_bytes` 让非 multipart 不依赖共享默认值，所以不需要形式上的 `max()`；如果未来把非 multipart 也迁到 `DefaultBodyLimit`，应明确写成 `max(multipart_raw_limit, NON_MULTIPART_BODY_LIMIT)` 或继续保留独立上限，不能因为某个较小的 `upload_max_bytes` 配置而把非 multipart 的契约上限也缩到 2 MiB 以下。

### 4. framing allowance 是框架开销余量，不是第二个数据预算

`MULTIPART_FRAMING_ALLOWANCE` 固定为 1 MiB（`src/server.rs:671`），公开契约也把它写成固定 allowance（`docs/contracts/config.md:80`）。它不应映射成用户配置，原因有三点：

1. 它不是用户可以提交的内容量，而是 part header、boundary 和分隔符造成的 wire 开销；把它做成第二个 TOML 数字会让“上传能放多少数据”变成两个旋钮的算术题。
2. 固定常量让安全边界保持简单：原始流最大就是 `upload_max_bytes + 1 MiB`，审计时只需要检查这一个公式。
3. 用户真正想扩大上传内容时，应该增大 `upload_max_bytes`；这会同时抬高数据预算和原始流上限，语义一致。

tradeoff: 1 MiB 是一个选定的有限 allowance，当前没有枚举验证常见客户端 framing 的实际大小；如果某个受支持客户端能构造超过 1 MiB 的合法 part header/name，原始流仍会被拒绝，需要单独评估常量是否该提高。当前不把这种极端值变成可配置项，是为了避免用配置绕过固定安全边界；升级应伴随协议级或安全级证据，而不是仅仅为了方便。

### 5. 保留非 multipart 的旧 413 形状

multipart 的解析失败走项目 JSON error envelope，包含 `request_id` 和错误类别（`src/server.rs:623`、`docs/contracts/config.md:80`）。非 multipart 超过 2 MiB 时走 `Handled::BodyTooLarge`，返回 413、空 body，不返回 multipart/script 的 JSON envelope（`src/server.rs:548-554`）。代码注释明确把它称为 pre-T12 行为，回归测试 `non_multipart_body_limit_stays_bounded` 构造 2 MiB + 1 字节请求并断言 413 且 body 为空（`tests/cli.rs:2221-2249`）。

这是有意的兼容决定，不是遗漏：T12 首版实现曾把非 multipart 超限改成新的 `request_body_too_large` JSON 公共错误类，评审判定那是超出 spec 的范围蔓延，随后回退（session history）。引入 multipart 的新错误 envelope 时，不应顺手改变原有非 multipart 客户端的响应形状。内部日志仍可使用 `request_body_too_large` 作为 class（`src/server.rs:553`），但客户端看到的是空 body；文档也明确区分这两类失败（`docs/contracts/config.md:80`）。

### 6. 测试必须同时覆盖两本账和两条路径

T12 当前回归覆盖了以下边界，建议后续改动沿用同一组形状：

- **数据预算恰好命中**：配置 64 字节，提交 64 字节 part content，返回 200。测试 helper 会加入 boundary、Content-Disposition、Content-Type 和 CRLF，因此实际 wire bytes 明显大于 64；能够成功依赖的正是 `64 + 1 MiB` 的原始流 allowance（`tests/cli.rs:507-526`、`tests/cli.rs:1901-1988`、`tests/cli.rs:1961-1964`）。
- **内容超预算**：65 字节 part content、无 Content-Length 的 chunked 请求、65 字节非文件字段，均在流式计数阶段返回 413 `upload_too_large`（`tests/cli.rs:1966-1976`、`tests/cli.rs:1979-1984`）。
- **声明长度预检**：声明 `limit + 1 MiB + 1`，在读取 body 之前返回 413 `upload_too_large`（`tests/cli.rs:1953-1960`、`tests/cli.rs:1979-1984`）。
- **只攻击 framing**：配置只允许 64 字节内容，但发送一个超长 part name，让 chunked multipart 的 wire bytes 超过 allowance；必须 413 `upload_too_large`，证明原始流闸门不是装饰（`chunked_multipart_framing_is_bounded`，`tests/cli.rs:2251-2275`）。
- **非 multipart 兼容**：2 MiB + 1 字节返回空 body 413（`tests/cli.rs:2221-2249`）。

这套测试比单纯“发送一个远超上限的 body 看 413”更强，因为它固定了“刚好等于数据预算仍可成功”和“只有 framing 超限也必须被拦住”这两个相反方向。

## Why This Matters

1. **避免把合法的配置值变成不可达承诺。** 如果原始流上限等于数据预算，用户配置 20 MiB 时，实际可用内容会少掉 boundary、header 和 CRLF 的开销；错误还会在应用账目运行前发生。分层后，`upload_max_bytes` 仍然表示用户可提交的 part content bytes，allowance 只服务于 wire framing。
2. **保留一个明确的安全上限。** 如果没有额外的 raw ceiling，超长 header/name 等 framing 可以绕过内容预算；如果直接把框架限制关闭，资源耗尽面更大。固定 allowance 使 multipart wire bytes 仍有硬边界，且这个边界可以写进契约和测试。
3. **让超限责任归属一致。** multipart 的 Content-Length 预检和流式解析都映射到同一个稳定类别 `upload_too_large`（`src/server.rs:116-133`、`src/files.rs:421-430`），调用方不必根据 413 的来源猜测错误形状；数据预算仍然是日志和诊断中的主账目。
4. **避免共享 Router 上限误伤其他请求类别。** 当前非 multipart 在应用层重新收紧到 2 MiB（`src/server.rs:210-211`、`src/server.rs:668`），因此不会因为 multipart 支持大文件而把旧路径一起放宽；反过来，也不能为了保持旧路径的 2 MiB 而让 multipart 的 20 MiB 配置失效。
5. **为后续演进留下正确接缝。** 本方案只限制字节；读取期限按这条接缝单独补上（#56 的 `server.request_timeout_ms`），叠加在“数据预算 / raw ceiling / 流式账目”之上，而不是把这几种约束重新合并成一个数字。

## When to Apply

- 应用层关心的是解码后的 payload 字节，而框架或解析器先看到的是带 framing、编码或传输开销的 wire bytes 时。
- 需要同时支持 multipart 和非 multipart，且两类请求有不同上限，但当前 Router 或 handler 入口只有一个共享限制时。
- multipart 等协议的 framing 可由客户端放大（超长 part name、header、boundary 或大量空 part），不能只依赖内容预算时。
- 使用 Content-Length 做快速拒绝，但它可能缺失或不准确，需要流式账目继续作为权威判断时。
- 新能力要保留旧请求类别的错误形状，不希望为了新增 JSON error envelope 而改变已有客户端行为时。

以下情况需要额外评估：合法客户端确实可能产生超过 1 MiB 的 framing；或者框架层限制已经能按路由/媒体类型独立配置，且不会与应用内匹配结果脱节。（请求体读取时间不再属于待评估项：它已由 #56 的 `server.request_timeout_ms` 限制。）

## Examples

### 例 1：默认配置下两个数分别是多少

```toml
[files]
root = "./files"
upload_max_bytes = 20971520  # 20 MiB，只计 part content
```

此时：

- 数据预算 = 20 MiB（`src/config.rs:690-692`、`docs/contracts/config.md:30`）。
- 原始流上限 = 20 MiB + 1 MiB = 21 MiB（`src/server.rs:363-376`、`docs/contracts/config.md:80`）。
- 20 MiB 的 file + form-field content 如果 wire framing 仍落在 21 MiB 内，应用账目可以通过。
- 20 MiB + 1 字节的 content 会在解析器累加 chunk 时失败并返回 413 `upload_too_large`（`src/files.rs:356-365`、`src/files.rs:297-301`）。
- content 不到 20 MiB、但 part header/name 等 framing 把整个 wire body 推过 21 MiB 时，框架层先拒绝，分类仍映射到 413 `upload_too_large`（`src/files.rs:421-430`、`chunked_multipart_framing_is_bounded`）。

### 例 2：小限额测试如何暴露“两个计数对象”

`multipart_limit_accepts_the_cap_and_rejects_precheck_chunked_and_form_fields` 把 `upload_max_bytes` 设为 64（`tests/cli.rs:1901-1913`）：

- 64 字节文件返回 200（`tests/cli.rs:1961-1964`、`tests/cli.rs:1961-1964`）。
- 65 字节文件、65 字节 chunked 请求、65 字节非文件字段都返回 413 `upload_too_large`（`tests/cli.rs:1966-1976`、`tests/cli.rs:1979-1984`）。
- 一个声明长度为 `64 + 1 MiB + 1` 的 multipart 请求由预检直接返回 413，不需要等待 body（`tests/cli.rs:1953-1960`、`tests/cli.rs:1979-1984`）。

第一个和后面几个用例的差异正是两层账目的价值：64 字节内容仍然合法，即使它周围的 multipart framing 让 wire body 超过 64；而 65 字节内容不论放在文件还是普通字段里，都会超过数据预算。

### 例 3：非 multipart 的第二条路径

非 multipart 请求不进入 `parse_multipart`，而是执行 `axum::body::to_bytes(..., NON_MULTIPART_BODY_LIMIT)`（`src/server.rs:210-211`）。即使共享的 Router 默认值为了 multipart 放宽到 20 MiB + 1 MiB，非 multipart 仍在 2 MiB 处被截断：

- `NON_MULTIPART_BODY_LIMIT = 2 * 1024 * 1024`（`src/server.rs:668`）。
- 2 MiB + 1 字节请求得到 413 空 body，不返回 JSON error envelope（`src/server.rs:548-554`、`tests/cli.rs:2221-2249`）。

因此“较宽的共享默认值”不等于“所有请求都放宽”；真正决定每条路径上限的是 handler 内按 multipart/non-multipart 分叉后选用的那本账。

## Related

- GitHub issue #53：T12 的 multipart 上传 spec，已关闭（随 PR #57 落地）。
- GitHub issue #51：v1 文件能力的父 spec，锁定 `upload_max_bytes` 的数据预算语义与 framing overhead 的表述。
- GitHub issue #56：`server: bound request body read time`，解析期读取期限（`server.request_timeout_ms`）；它与本文的字节分层是两个独立约束，不能互相替代。
- 相邻 convention：`docs/solutions/conventions/host-boundary-fail-closed-input-classification.md`，其中已预测 T12 会检验“缺失、畸形、超限不可压平”的边界。
- 同一批 multipart 夹具的测试侧规则：[hold-upload-temp-dir-with-unfinished-request-body.md](../test-failures/hold-upload-temp-dir-with-unfinished-request-body.md) —— 请求级上传临时目录如何被确定性地观察；本文负责字节分层，那篇负责测试同步。
- 配置契约：`docs/contracts/config.md:30`、`docs/contracts/config.md:80`。
- `ctx` 契约：`docs/contracts/ctx-api.md:71-73`（非 multipart 暴露 `[]`，非文件字段仍计入 `files.upload_max_bytes`）。
- 安全边界：`SECURITY.md:23`，明确“整个 multipart body stream，including framing，受 `upload_max_bytes` 加 1 MiB allowance 约束，数据预算不含 framing，流式账目为权威”。
- 回归测试：`tests/cli.rs:1901-1988`（预算命中、超限、chunked、普通字段、声明长度预检）与 `tests/cli.rs:2251-2275`（纯 framing 超限）；非 multipart 兼容见 `tests/cli.rs:2221-2249`。

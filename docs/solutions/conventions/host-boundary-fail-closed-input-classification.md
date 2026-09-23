---
title: Keep host-boundary failures distinct from absence and validate before blocking
date: 2026-09-23
category: conventions
module: host boundary input validation
problem_type: convention
component: server
severity: medium
applies_when:
  - "Adding or reviewing a host capability that must distinguish absent, malformed, and unusable input"
  - "Parsing HTTP request fields such as `Range`, especially repeated field lines or non-UTF-8 values"
  - "Opening untrusted local paths where special files or blocking system calls cross the script host boundary"
  - "Designing T12 multipart upload validation and its malformed-request, size-limit, and error-mapping tests"
  - "Writing black-box end-to-end tests for fail-closed host boundaries"
symptoms:
  - "Two `Range` field lines are folded into the first value, so a multi-range request gets 206 instead of the contracted 416"
  - "A raw non-UTF-8 `Range` value is treated as no Range header, returning the full 200 response"
  - "A `Range` spec with internal whitespace such as `bytes=0 - 3` is accepted instead of rejected as malformed"
  - "Opening a root-contained FIFO blocks until the script deadline and surfaces as 500 `script_error` instead of a catchable `file_io_error`"
root_cause: missing_validation
resolution_type: code_fix
related_components: [script, files, tests]
tags: [host-boundary, fail-closed, missing-vs-unusable, range, ctx-file, fifo, non-blocking-open, end-to-end-tests]
---

# 宿主边界输入：先把“缺失”和“畸形”分开，再决定可用状态

## Context

Stunt Double 是宿主注入 `ctx` 的 mock server。issue #52（T11）实现 `ctx.file.readText`、`ctx.file.readBytes`、`ctx.file.stream` 的根目录受限读取，以及本地文件流的单 Range 响应。当前工作分支是 `feat/52-rooted-file-reads`；修复已在本地分支完成，但分支尚未推送、没有已创建的 PR，也尚未合并到 `main`，因此本文不能当作 main 上的既成状态。

落地后的两轮对抗性黑盒评审又发现 4 个同类缺陷：

（第 1、2 条来自会话中的黑盒复现，对应旧实现只存在于未提交的工作树中间态，没有可保留的提交证据；第 3、4 条可在本分支的提交历史中复核。）

1. 两个 `Range` 字段行（例如第一行 `bytes=0-1`、第二行 `bytes=3-4`）被当成单 Range，返回 206 和第一段 body。HTTP 语义上重复字段行等价于逗号合并后的多 Range；本能力不支持多 Range，正确的可观察结果是 416、`Content-Range: bytes */<size>`、空 body。
2. 原始非 UTF-8 的 Range 值 `bytes=\x80` 经宽松的 `to_str().ok()` 折叠成“没有 Range”，结果返回 200 全量，而不是 416。
3. 根目录内 FIFO 会阻塞 `File::open`。读取路径先 open、后检查 `metadata.is_file()`，无写端 FIFO 会一直阻塞到脚本超时，最终变成 500 `script_error`；类型检查必须发生在可能阻塞的 open 之前。
4. Range 解析曾 trim unit、spec 及两端数字，`bytes=0 - 3` 因此被当作合法单 Range 返回 206。RFC 9110 的 range-spec 不允许这种内部空白，应返回 416。

这四条不是四个互不相关的偶发 bug，而是同一条 host-boundary 规律：原始协议输入有“缺失、合法单值、畸形/复合”三种状态，文件系统对象也有“普通文件、目录、FIFO、socket、设备”等类型；如果过早把状态压成 `Option` 或先执行可能阻塞/有副作用的调用，宿主就会静默答错或挂住。按设计预测，T12 的 multipart 上传会碰到同一形状：Content-Length 缺失与值超限、multipart 结构畸形与体积超限、临时文件写入限制都不能混成一个“没有值/解析失败”的分支。因此这份记录按 knowledge track 的 practice 文档化，而不是只记录某次补丁。

(session history) 更早的会话已经为这条规则提供了先例与反例：T11 规划明确否决了"非法 `Range` 一律忽略并返回 200"的备选方案，理由是调用方无法区分"服务端不支持区间"与"请求区间语法坏了"；T6 则把 catch-all 的错误归类拆成 `upstream_url_invalid` / `upstream_redirect_error` / `upstream_unreachable` / `upstream_http_error` / `script_error`，因为压平会丢失可操作的边界语义。这两次决策都把"畸形输入必须走显式路径"当作可预测性要求，而不是风格偏好。

## Guidance

### 1. 用显式状态保留“缺失”和“畸形”的区别

当前实现把 Range 请求解析为三态，而不是 `Option<&str>`：

```rust
enum RangeRequest<'a> {
    None,
    Single(&'a str),
    Unusable,
}
```

解析时 `headers.get_all("range")` 用来保留重复字段信息：没有字段是 `None`，恰好一个且 UTF-8 合法才是 `Single`，多个字段行或 `to_str()` 失败都是 `Unusable`（`src/server.rs:512-533`）。`Unusable` 再明确映射为 `RangeDecision::Unsatisfiable`（`src/server.rs:451-456`），最终生成 416、`bytes */<size>`、`Content-Length: 0` 和空 body（`src/server.rs:487-498`）。

关键不是枚举名字，而是决策顺序：先确认原始形态是否还能表达成一个受支持的单值，再做业务默认值。`None` 才会进入“无 Range”的 200 路径；畸形输入不能借用该默认值。Range 能力本身的公开契约写明：不可满足、畸形、多 Range 或空文件上的 Range 都答 416（`docs/contracts/ctx-api.md:67`；中文合同同样规定于 `docs/contracts/ctx-api.zh-CN.md:69`）。

这里还要区分两个“重复字段”概念。`ctx.request.headers` 快照对重复名字采用 last-wins，是脚本可见的数据契约（`docs/contracts/ctx-api.md:40`）；它不能替代宿主对 Range 等 framing 字段的协议解析。文件响应直接接收请求 `HeaderMap`（`src/server.rs:419-424`），并按 HTTP 字段语义处理重复行。

### 2. 复合字段先按协议语义展开，再判断能力子集

重复的 `Range` 字段行在 HTTP 中不是一个字段的“第二个值”，而是一个逗号列表。宿主当前只支持单 Range，所以只要出现多个字段行，就不能偷偷选第一行来满足 206；它应当先归入 `Unusable`。同理，已经放在一个字段值内的 `bytes=0-1,3-4` 由 `decide_range` 的 `spec.contains(',')` 拒绝（`src/files.rs:403-418`）。这两条路径最终都收敛到同一个 416，而不是分别给出“第一个范围成功”和“多范围失败”两种行为。

语法校验也必须停在协议允许的边界：字段值外层 OWS 可以去掉，但进入 unit、spec 和索引数字后不能再 trim。当前 `decide_range` 只对完整 header 调用一次 `.trim()`；`unit` 必须精确忽略大小写等于 `bytes`，`parse_index` 只接受非空且全为 ASCII 数字的字符串（`src/files.rs:403-418`、`src/files.rs:455-460`）。因此 `bytes=0 - 3`、`bytes= 0-3`、`bytes=0- 3` 都不会被宽容成 206，而会进入 416。

### 3. 可能阻塞的调用前先检查对象类型，调用后再复核

文件路径不能只“成功 resolve 就算打开”。当前 `open` 的顺序是：

1. `resolve` 得到 canonical target；
2. 先对 target 做 `std::fs::metadata`，非普通文件立即返回 `Error::Io("path is not a regular file")`；
3. 只有通过该检查才调用 `File::open`；
4. 对已打开的文件再做一次 `file.metadata().is_file()` 检查；
5. 最后执行 `verify_open_target`，在 Linux 上通过 `/proc/self/fd` 复核实际打开对象仍在根内（`src/files.rs:294-312`、`src/files.rs:344-373`）。

这条顺序让 FIFO、目录、socket 等对象在触达可能阻塞的 open 前被拒绝。`Error::Io` 的脚本可见错误码是 `file_io_error`（`src/files.rs:66-73`），与合同对非普通文件的定义一致（`docs/contracts/ctx-api.md:63`）。这是可捕获错误，不是脚本超时，也不是静默读取成功。

前后两次 stat/open 检查并不消除 TOCTOU：本地写者仍可在检查与 open 之间替换目录项。当前模型把这件事明确记为本地半信任模型的接受取舍，Linux 用已打开 fd 的复核缩小窗口，非 Linux 仅保留 pre-open 检查；升级路径是 `openat` 风格解析加 `O_NOFOLLOW`（`src/files.rs:7-10`、`SECURITY.md:19`）。因此修复的目标不是宣称“无竞态”，而是让普通 FIFO 不再阻塞，并保持已接受的竞态边界可见。

### 4. 回归测试要走传输层原始形态，而不是只调用便利 API

便利的 `request` helper 可以表达重复 header，但无法表达非法 UTF-8 的 header 字节；因此 `ctx.file.stream` 的 416 用例使用 `request_raw_range` 手工写 HTTP/1.1 请求字节（`tests/cli.rs:335-350`）。这很重要：如果测试层先经过客户端/字符串类型，`bytes=\x80` 可能在到达 server 前就被拒绝或替换，测试就永远覆盖不到实际的 HeaderValue 解析边界。

同一原则适用于阻塞对象：FIFO 测试用 `mkfifo` 创建真实对象，并把 sandbox 超时缩短到 500 ms；修复前它会挂到脚本 deadline，修复后必须快速返回可捕获的 `file_io_error`（`tests/cli.rs:1924-1948`）。测试不是断言“某个 helper 返回某个内部枚举”，而是断言客户端最终看到的状态、header 和 body。

## Why This Matters

这些缺陷都不是崩溃，而是“静默答错”：重复 Range 返回一个看似合法但语义错误的 206，非 UTF-8 Range 回落成 200 全量，内部空白被当作 206，FIFO 则把类型错误变成脚本超时。调用方最难发现这种问题，因为 HTTP 状态和 body 都像是成功的；错误会在 mock 下游被当成真实协议行为继续传播。

范围解析和文件打开都属于安全与资源边界。把重复/畸形输入折叠成缺失，可能绕过 fail-closed 分支；把类型检查放到阻塞 open 之后，则让一个输入形态直接占满脚本 deadline。相对地，416、空的错误 body、可捕获的 `file_io_error` 都是稳定且可断言的失败面，能让路由脚本决定是否转成业务响应。

(session history) 这一习惯在本仓库有连续先例：T1 的配置解析要求失败时给出 dotted 字段路径、期望形状与实际值，并且只在 `TcpListener::bind` 成功后报告 listening；T5 也把"非 2xx、JSON 不可解析、缺 `data`、传输失败"显式映射为 502，同时让策略拒绝保持 500。本次记录只是把同一标准扩到 HTTP framing 字段与文件系统对象类型。

T12 会把同一风险带到上传路径。按产品定义，上传由宿主持有 multipart 解析、默认 20MB（可配置）大小上限、每请求独立临时目录并在请求结束清理（`plans/product-definition.md:31`）；当前契约仍把上传大小限制列为待 T12 落地（`docs/contracts/ctx-api.md:89`）。上传实现必须避免“缺 Content-Length 就当 0”“multipart 畸形与超限都当 400 或都当 413”“临时文件写满后继续读入内存”这类同类降级。本文件中的“缺失/合法/畸形”三态和“执行前先检查对象/资源类型”适用于该切片，但本次会话没有在 T12 代码上验证这些上传结论。

此外，错误可见性影响可观测性：文件调用日志需要完成记录，脚本才能捕获错误并继续；如果 FIFO 阻塞到全局脚本超时，`file_calls` 的完成记录也不能按正常路径收尾。当前 `open_stream` 的失败分支会结束该次文件调用并记录错误码（`src/files.rs:209-220`），这与快速、可分类的失败目标一致。

## When to Apply

- 新增或修改任何从 `HeaderMap` 读取的 framing/协议字段时：`Range`、`If-Range`、HTTP multipart 的 `Content-Type` boundary、`Content-Length`、`Content-Range` 等。先列清“缺失、单个合法值、重复字段、合并列表、非法字节、语法畸形”各自的可观察结果。
- 实现 T12 上传限制时：区分 Content-Length 缺失与显式 0；区分声明长度超限与实际流式字节超限；区分 multipart 结构畸形（通常是 400 类输入错误）与实体过大（通常是 413 类限制错误）；同时为临时文件设独立上限并保证清理。
- 读取任何可能是 FIFO、目录、socket、设备或符号链接的路径时：在阻塞 open 前做对象类型检查，open 后再复核；把残余 TOCTOU 明确记录在设计/安全文档，而不是默默忽略。
- 设计缓冲与流式资源上限时：缺少长度不代表没有数据；`None` 不能直接转换为 0 或“无限制”。声明长度也不能替代实际读取计数。
- 为边界写回归测试时：至少覆盖空/缺失、一个合法值、重复字段、非 UTF-8 原始字节、内部空白、超限和会阻塞的对象。对无法由普通客户端构造的输入，直接构造传输层请求。
- 做下一轮对抗性 review 时：优先寻找“第一值/最后值/默认值”这类会让畸形输入静默走成功路径的折叠，以及任何在验证前就执行且可能阻塞的系统调用。

## Examples

### 例 1：重复 Range 与非 UTF-8 Range 不再折叠为缺失

下面是从本轮会话评审复现中抽象出的修复前等价形态；旧工作树代码没有保留在当前 HEAD，因此这里只把旧行为作为“本次会话结论”，不把它写成当前源码。它把缺失、重复字段和非 UTF-8 都压成了 `Option`：

```rust
// Before（本次会话结论）：缺失、重复、非 UTF-8 最终都只能表达成 Option
let range = headers.get("range").and_then(|value| value.to_str().ok());
let decision = files::decide_range(file.size, range);
```

按会话中的复现，它会产生两种静默错误：两个 Range 字段行只取到第一个 `bytes=0-1`，于是回答 206 和 `01`；`bytes=\x80` 则成为 `None`，于是回答 200 全量。

当前树改成三态并保留重复信息：

```rust
enum RangeRequest<'a> {
    None,
    Single(&'a str),
    Unusable,
}

fn range_request(headers: &HeaderMap) -> RangeRequest<'_> {
    if headers.contains_key("if-range") {
        return RangeRequest::None;
    }
    let mut values = headers.get_all("range").iter();
    let Some(first) = values.next() else {
        return RangeRequest::None;
    };
    if values.next().is_some() {
        return RangeRequest::Unusable;
    }
    match first.to_str() {
        Ok(value) => RangeRequest::Single(value),
        Err(_) => RangeRequest::Unusable,
    }
}
```

当前实现见 `src/server.rs:512-533`；`Unusable` 到 416 的映射见 `src/server.rs:451-456`，最终响应见 `src/server.rs:487-498`。公开契约要求不可满足、畸形、多 Range 或空文件 Range 答 `416`、`Content-Range: bytes */<size>`、无 body（`docs/contracts/ctx-api.md:67`）。回归测试 `ctx_file_stream_answers_416_for_unusable_ranges` 在 `tests/cli.rs:1682`，其中 `tests/cli.rs:1697-1708` 发送不可满足、非数字、逗号多 Range、空文件、重复字段行、非 UTF-8 原始字节和内部空白，`tests/cli.rs:1711-1728` 对七种情况逐一断言 416、正确的 `Content-Range`、`Content-Length: 0` 和空 body。非 UTF-8 请求由 `request_raw_range` 发送（`tests/cli.rs:335-350`）。

### 例 2：Range 内部空白必须保持为畸形

修复前（本次变更 diff 中可复核的旧顺序）会对 unit、spec 和两端数字分别 trim：

```rust
// Before（历史实现）：内部空白被吃掉
let Some((unit, spec)) = header.trim().split_once('=') else { /* 416 */ };
let spec = spec.trim();
if !unit.trim().eq_ignore_ascii_case("bytes") { /* 416 */ }
let (first, last) = (first.trim(), last.trim());
```

因此 `bytes=0 - 3` 会变成 `0` 和 `3`，错误地返回 206。当前树只在字段值外层做一次 trim，后续 unit 和数字按原字符串校验：

```rust
let Some((unit, spec)) = header.trim().split_once('=') else {
    return RangeDecision::Unsatisfiable;
};
if !unit.eq_ignore_ascii_case("bytes") || spec.contains(',') || size == 0 {
    return RangeDecision::Unsatisfiable;
}
// ...
fn parse_index(raw: &str) -> Option<u64> {
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    raw.parse().ok()
}
```

当前实现见 `src/files.rs:403-418` 与 `src/files.rs:455-460`。该行为由同一个 `ctx_file_stream_answers_416_for_unusable_ranges` 用例的 `bytes=0 - 3` 分支固定（`tests/cli.rs:1708`），并与合同中的 malformed Range 结果一致（`docs/contracts/ctx-api.md:67`）。

### 例 3：FIFO 在 open 前被拒绝，同时保留已声明的 TOCTOU 取舍

修复前的等价顺序是：

```rust
// Before（历史实现）：File::open 可能先阻塞；metadata 检查来得太晚
let file = File::open(&target)?;
let metadata = file.metadata()?;
if !metadata.is_file() {
    return Err(Error::Io("path is not a regular file".to_string()));
}
```

无写端 FIFO 会卡在 `File::open`，脚本直到 deadline 才失败。当前树把类型检查前置，并在 open 后复核：

```rust
let metadata = std::fs::metadata(&target).map_err(|error| map_open_error(&error))?;
if !metadata.is_file() {
    return Err(Error::Io("path is not a regular file".to_string()));
}
let file = File::open(&target).map_err(|error| map_open_error(&error))?;
let opened = file.metadata().map_err(|error| Error::Io(error.to_string()))?;
if !opened.is_file() {
    return Err(Error::Io("path is not a regular file".to_string()));
}
self.verify_open_target(&file, &root)?;
```

当前实现见 `src/files.rs:294-312`。非普通文件映射为 `file_io_error`，与合同一致（`src/files.rs:66-73`、`docs/contracts/ctx-api.md:63`）。回归测试 `ctx_file_rejects_non_regular_files_without_blocking` 在 Unix 上用 `mkfifo` 创建无写端 FIFO，并用 500 ms sandbox deadline 断言快速返回 `file_io_error`（`tests/cli.rs:1924-1948`）。这次修复没有消除剩余 TOCTOU；该窗口仍按 `src/files.rs:7-10` 与 `SECURITY.md:19` 记录的本地半信任取舍保留。

### 例 4：相关但独立的路径解释边界

本轮还固定了一个相邻问题：URL 风格路径和平台特定路径不能被当前宿主误解析为系统路径。`ctx_file_never_resolves_url_style_or_platform_specific_names_outside_the_root` 验证 `file:///etc/passwd`、`C:\Windows\win.ini` 和 `..\..\secret` 在各平台都只能按根内相对名或路径规则处理，不能借路径风格逃逸（`tests/cli.rs:1952-1983`）。这不属于前面四种 Range/FIFO 缺陷，但它强化了同一个边界习惯：先按本能力的路径模型解释，再讨论对象是否存在；不要让字符串形状隐式切换到另一套语义。

### 验证状态

当前分支 `feat/52-rooted-file-reads`（issue #52）已包含这四项修复；分支尚未推送，也尚未创建或合并 PR。本次在 current HEAD 运行 `cargo test --workspace --all-features`，结果为 19 个 lib 单测加 120 个 `tests/cli.rs` 端到端测试，共 139 passed、0 failed；其中新增回归覆盖重复/非 UTF-8/内部空白 Range、FIFO 非阻塞拒绝，以及路径风格逃逸边界。

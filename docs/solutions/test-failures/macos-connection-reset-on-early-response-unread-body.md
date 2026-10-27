---
title: "Tolerate client body write reset when testing early server responses with unread bodies"
date: 2026-10-26
category: test-failures
module: fallback route integration test
problem_type: test_failure
component: testing_framework
symptoms:
  - "E2E tests sending an oversized or partial request body panic during `write_all` with `Connection reset by peer` (ECONNRESET) or `Broken pipe` (EPIPE) on macOS/BSD."
  - "The test fails before `read_response` can inspect the HTTP response, even though the server correctly processed the request and sent an early response."
root_cause: tcp_rst_on_unread_body
resolution_type: test_fix
severity: medium
last_updated: 2026-10-27
related_components:
  - server
  - tests
  - cli
tags: [macos, connection-reset, econnreset, broken-pipe, tcp-rst, early-response, unread-body, fallback, e2e-tests, integration-test]
---

# 服务端提前响应且不读 Body 时，测试客户端需容忍写入重置（ECONNRESET / Broken pipe）

## 第二次命中（chunked 上传路径，PR #202）

同一根因的第二个调用链：`chunked_multipart_framing_is_bounded` → `request_multipart_chunked` → `request_chunked_bytes`（`tests/cli.rs`）。该 helper 写侧已用 `let _ = stream.write_all(...)` 容忍 RST，但读侧仍在 `read_response`（`read_to_end` 等 EOF）上 panic——服务端对未读取的 chunked body 提前回 400/413 并关闭连接，macOS 上 close 因接收缓冲仍有未读字节转为 RST，`read_to_end` 丢掉已收到的完整 413 信封并报 `Os { code: 54, kind: ConnectionReset }`。修复方式与本文方案一致：读侧改用 `read_response_until_complete` 按 `Content-Length` 定界，截断响应仍判失败，413 断言保持原义。

## 问题

在为 `[fallback]` 路由编写 E2E 测试时，需要验证「未知路径无需读取或等待请求体，即便客户端发送超大 Body 也必须立即返回 404」。然而在 macOS（Darwin/BSD 协议栈）上运行测试时，客户端如果在服务端关闭连接后仍继续向 socket 写入请求体，`TcpStream::write_all` 会遭遇 `Connection reset by peer`（os error 54 / `ECONNRESET`）或 `Broken pipe`（os error 32 / `EPIPE`）直接 panic，导致测试在验证服务端已发出的 404 响应之前夭折。

## 症状

- 测试用例执行到 `stream.write_all(partial).expect("write body")` 时直接 panic 崩溃。
- panic 报错为 `Connection reset by peer` 或 `Broken pipe`，但在服务端日志中请求已被正常处理并完成了 404 响应投递。
- Linux 下可能因为 socket 缓冲区容量或 TCP 实现差异暂时侥幸通过，而在 macOS / Apple Silicon runner 上稳定触发写入崩溃。

## 试过但无效的做法

- **直接复用现有的 `request_partial_body` 助手**：原 `request_partial_body` 对请求头和请求体均使用 `stream.write_all(...).expect("write body")`。在常规超时的测试（如 408）中，客户端只发一小段数据后就不再写入，因此没有后续写入触发 RST；但在 fallback 超大请求体测试中，客户端一次性写入 5MB 数据，服务端提前写回 404 并关闭 socket，导致客户端剩余写入必然遭遇 RST，测试必挂。
- **调大套接字缓冲区或在客户端 `sleep` 后再读响应**：无法解决根本问题；只要服务端关闭 socket 时接收缓冲区中仍有未读取的字节，TCP 协议栈就会发 RST，客户端后续的写入永远是不可靠的。
- **让服务端在关闭连接前强制 drain 请求体**：违背业务契约与设计初衷。Fallback 路由的设计目标正是零开销、不读 Body、防 DoS，如果为了迎合测试强行在服务端消耗带宽读取未知路径的垃圾数据，将引入不必要的资源开销和安全隐患。

## 解决方案

在 `tests/cli.rs` 中引入专用的宽松写入测试助手 `request_partial_body_lenient`：

1. **请求头正常写入并断言**：`stream.write_all(head.as_bytes()).expect("write head");` 确保请求头顺利发出。
2. **请求体写入忽略错误**：`let _ = stream.write_all(partial);`。因为服务端不读 Body 并提前完成响应关闭 socket，写入可能因竞争失败被重置，忽略此处的写入错误。
3. **继续读取并定界响应**：调用已有的 `read_response_until_complete(&mut stream)`。该函数按 `Content-Length` 解析完整响应而无需等待 EOF，即使连接被重置或关闭，只要接收缓冲已有完整的 HTTP 头和响应体，即可成功完成断言。
4. **精确断言 Fallback 语义**：在 `fallback_oversized_body_still_answers_404` 中使用 5MB 大包调用 `request_partial_body_lenient`，断言状态码为 404，验证服务端在不读 Body 的前提下提前关闭且正确响应。

## 为什么这样可行

- **TCP 协议栈标准行为（RFC 1122 §4.2.2.13）**：当一个 TCP 连接在接收缓冲区中还有未读取的数据（unread data in socket receive buffer）时被应用层调用 `close()`，TCP 协议栈必须发送 RST（Reset）段，丢弃接收缓冲区的数据，而不是发送正常的 FIN 报文执行四次优雅关闭。
- **macOS 与 Linux 的差异**：在 macOS（Darwin/XNU）协议栈中，只要对端发送了 RST，本地 socket 的写操作会极其灵敏地立即报错 `ECONNRESET` 或 `EPIPE`；Linux 在部分场景下如果写缓冲区未溢出可能先返回写入成功，但若收到 RST 同样会在下一次系统调用中报错。
- **测试视角的真正目标**：该测试验证的是「服务端是否在未读请求体的情况下成功响应了 404 并拒绝继续处理」，而不是「客户端能否完整上传无用数据」。因此，客户端在 Body 写入阶段遭遇 RST 恰恰是服务端未读取 Body 就关闭连接的物理证据；忽略写入错误并读取已接收到的响应是语义完全自洽且跨平台稳定的做法。

## 预防

- **区分「断流测试」与「全量传输测试」**：当测试服务端的主动提早终止行为（如 400 Bad Request、404 Fallback、413 Payload Too Large、408 Timeout、401 Unauthorized 等），如果测试打算发送超出服务端读取意愿的数据，客户端写入绝不能假设连接会保持打开。
- **测试助手命名表达意图**：命名为 `request_partial_body_lenient`，并在函数注释中清晰标注 `ignores a failed body write: a server that closes the socket early ... resets the connection`。
- **善用 `read_response_until_complete`**：避免无脑 `read_to_end`，优先使用按 `Content-Length` 定界或逐行解析的读取逻辑，以便在套接字被重置前读出已经到达内核缓冲区的有效响应报文。

## 相关

- GitHub issue #173：Scriptable fallback route for unknown paths
- PR #198：`feat(config): add scriptable fallback route for unknown paths`
- PR #200：`docs: document fallback contract and align script error semantics`
- 关联测试代码：`tests/cli.rs:509-528`（`request_partial_body_lenient` 实现），`tests/cli.rs:7245`（`fallback_oversized_body_still_answers_404`）
- 第二次命中（chunked 上传路径，#202）：`tests/cli.rs` 的 `request_chunked_bytes` 写侧已用 `let _ = stream.write_all(...)` 容忍 RST，但读侧仍在 `read_response`（`read_to_end` 等 EOF）上 panic——服务端对未读取的 chunked body 提前回 400/413 并关闭连接，macOS 上 close 因接收缓冲仍有未读字节转为 RST，`read_to_end` 丢掉已收的完整 413 信封并报 os error 54。修复方式与本文方案一致：读侧改用 `read_response_until_complete` 按 `Content-Length` 定界，截断响应仍判失败，413 断言保持原义。
- 类似套接字与异步时序学习：`docs/solutions/test-failures/hold-upload-temp-dir-with-unfinished-request-body.md`
- 请求体截止与读取边界：`docs/solutions/conventions/request-read-deadline-at-the-connection-layer.md`
- 客户端侧信封必达的上限（服务端拒绝路径的有界 lingering drain，nginx `lingering_close` / Go `net/http` 256 KiB 先例）作为产品级增强挂账 issue #204，含触发条件与外链案例（hyper#3078、rift#1460、rustfs#351、nanocached#775）；当前无用户报告信封丢失，不实现。

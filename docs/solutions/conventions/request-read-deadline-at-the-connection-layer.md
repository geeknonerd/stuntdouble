---
title: "Arm an inbound read deadline on the protocol builder, not the auto-detecting wrapper"
date: 2026-09-23
category: conventions
module: server inbound request deadlines
problem_type: convention
component: server
severity: medium
applies_when:
  - "为入站连接增加读取期限（请求头、请求体或空闲超时）时"
  - "在 axum::serve 与直接使用 hyper / hyper-util 构建连接循环之间做选择时"
  - "写慢客户端回归测试（半写请求头、半写请求体）时"
  - "评审会改变依赖 feature 的连接层改动时"
related_components: [server, tests]
root_cause: incomplete_setup
resolution_type: code_fix
tags: [hyper, hyper-util, axum, header-read-timeout, request-timeout, http1, http2-preface, slowloris, end-to-end-tests]
---

# 入站读取期限要挂在协议 builder 上，而不是自动探测的包装层

## 背景

issue #56 要求 `serve` 用一个期限同时覆盖请求头与请求体（`server.request_timeout_ms`）。实现前有两件事必须读源码才能确认：

1. **默认值存在 ≠ 默认值生效。** hyper 1.11 的 `http1::Builder` 自带 30 秒 `header_read_timeout` 默认值，但它只在注册了 timer 之后才生效；`axum::serve` 搭建连接循环时既不设期限也不设 timer，所以这个默认值一直是惰性的。这也是「`serve` 没有请求读取期限」的直接原因（本仓库 `src/server.rs` 的 383 行起记录了这段推理）。
2. **自动探测的包装层会把期限挡在外面。** 用 hyper-util 的 `auto::Builder`（axum 内部用的那个）可以拿到该 builder，但代价有两个：`server-auto` feature 会启用 `http2`，把 `h2` 拉进依赖树，而 v1 只承诺 HTTP/1.1；更隐蔽的是 auto 在建立 HTTP/1 连接前会先跑 `read_version`，无期限地最多读 24 字节判断 HTTP/2 preface——短于此长度的半写请求头完全不受 `header_read_timeout` 约束。

第 2 点被两轴 review 独立指出，而当时的回归（发送约 50 字节的半写请求头）恰好通过：它已经越过 24 字节嗅探窗口。回归现已改为只发 1 字节（本仓库 `tests/cli.rs` 的 4835 行起）。

## 指南

- 用 hyper 的 HTTP/1 builder 建连接，并把 timer 与期限一起设上：

```rust
let mut builder = ConnectionBuilder::new(); // hyper::server::conn::http1::Builder
builder
    .timer(TokioTimer::new())
    .header_read_timeout(Some(head_deadline));
```

  连接借用 builder，所以连接任务持有 builder 的克隆（`Builder: Clone`）并在任务内构造连接；graceful shutdown 仍通过对在途连接调用 `Connection::graceful_shutdown` 保持排空语义（本仓库 `src/server.rs` 的 390 行起）。

- hyper-util 只保留 `TokioIo`、`TokioTimer`、`TowerToHyperService` 所需的最小 feature；本仓库收敛为 `["service", "tokio"]`（`Cargo.toml`）。`axum::Router` 本身实现了 `Service<Request<B>>`，其中 `B` 可以是 `hyper::body::Incoming`，所以它可以直接交给 `TowerToHyperService`，不需要额外适配层，也不需要把 `tower` 变成直接依赖。
- 不要把期限挂在自动探测层上；任何在协议判定阶段先读字节的包装层都会让期限只覆盖判定之后的连接。

## 为什么重要

- 惰性默认值在任何依赖里都可能存在，只有读「构造这个对象的那段代码」才能确认它是否真的生效；类型和文档字符串不会提示。
- 嗅探盲区会让保护看起来已经完成：半写请求头仍可无限占用连接，而一个「够长」的测试请求头会给出虚假的绿色。
- feature 传染会顺带扩大协议面与供应链：引入 auto 就等于把 http2 编译进来并暴露入口，即使产品只承诺 HTTP/1.1（`Cargo.lock` 里多出的 `h2`、`fnv` 就是信号）。

## 何时适用

- 为入站连接添加或修改读取期限时。
- 在 `axum::serve` 与自建 hyper / hyper-util 连接循环之间选择时。
- 写慢客户端回归时：从「一个字节都不发」或「只发 1 字节」起步，再补完整请求头，才能覆盖协议探测层。
- 评审触及依赖 feature 的连接层改动时：核对 `Cargo.lock` 是否新增了本不需要的 crate。

## 示例

- 错误做法与它的绿色测试：auto builder + 约 50 字节半写请求头 —— 测试通过，但只发 1 字节的客户端仍可无限占用连接。
- 正确做法与回归：`tests/cli.rs` 的 4835 行起断言 1 字节请求头在期限后收到连接关闭且日志出现 `request_head_timeout`；4758 行起与 4798 行起覆盖慢请求体，包含 408 envelope、请求日志类别、announced `Content-Length` 与上传临时目录的清理。

## 相关

- `src/server.rs` —— 连接循环、期限与连接级日志类别的实际位置
- `Cargo.toml` —— hyper / hyper-util 的 feature 选择
- `docs/contracts/config.md`、`docs/contracts/cli.md`、`SECURITY.md` —— 期限与错误/日志类别的公开契约
- issue #56（`server: bound request body read time`）—— 本文的实现来源；分支 `fix/server-body-read-timeout`，截至本文写作时尚未合并

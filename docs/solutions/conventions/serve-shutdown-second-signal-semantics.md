---
title: "A second shutdown signal is only guaranteed after the first one is observed"
date: 2026-09-22
last_updated: 2026-09-23
category: conventions
module: serve shutdown lifecycle
problem_type: convention
component: server
severity: medium
applies_when:
  - "Changing serve signal handling or its exit-code contract"
  - "Writing end-to-end tests that send SIGINT/SIGTERM or Windows Ctrl-C"
  - "Reviewing claims that a second shutdown signal always forces an immediate exit"
related_components: [server, cli, tests]
tags: [shutdown, signals, sigint, sigterm, ctrl-c, exit-codes, end-to-end-tests, windows]
---

# Serve 的第二次关闭信号只在第一次被观测后才有保证

## 背景

T10（issue #33，分支 `feat/graceful-shutdown`）为 `serve` 增加信号驱动的 graceful shutdown。信号安装留在进程边界：`src/main.rs:86-90` 创建 shutdown future 后把它交给 `server::run`；`src/server.rs` 的 390 行起的 `serve_connections` 把该 future 接在自己的连接循环上：首个信号后停止 accept，通知每个在途连接调用 `Connection::graceful_shutdown`，再等待所有连接任务结束（该循环在 #56 中从 `axum::serve` 换成 hyper 的 HTTP/1 builder）。Unix 同时预注册 SIGINT/SIGTERM 两个信号流，`recv_shutdown_signal` 用 `tokio::select!` 接收第一个通知，首次信号触发正常 drain，后台任务继续等待第二个，并在观测到后按 128+signal 立即退出（`src/main.rs:124-152`）。Windows 使用预注册的 `tokio::signal::windows::ctrl_c()` 流，第二次 Ctrl-C 退出 `130`（`src/main.rs:155-167`）。

公开契约是：第一个信号停止接受新连接、排空在途请求并以 `0` 返回；只有在第一次关闭信号被观测后再次观测到第二个信号，才保证放弃排空并立即退出，SIGINT/Ctrl-C 为 `130`，SIGTERM 为 `143`（`docs/contracts/cli.md:43-45`；`plans/adr/0011-contract-compatibility.md:45-46`）。

初稿曾把后一半写成“第二个信号总是立即终止”。本次会话的人工复现使用 30 秒后才返回的上游响应并保持客户端连接：连续发送两次 SIGTERM 后，stderr 只有一次 `starting graceful shutdown`，1 秒后进程仍在 drain，没有出现强制退出。原因不是缺少第二个监听任务，而是标准（非实时）信号不排队：同一信号的前一次通知尚未被消费时，后续投递可能被合并成一个通知。早期测试在两个信号之间固定 `sleep(200 ms)`，让第一次信号先有机会被观测，因而掩盖了该竞态（人工复现与旧测试属于会话记录；最终契约见上述文档）。

因此 T10 明确选择：契约只承诺“第一次 shutdown 已启动之后”的第二个信号；背靠背信号若被合并，就按第一次信号正常排空并退出 `0`。不要为了强迫计数引入 signal-hook 或实时信号。迁移影响记录在 `CHANGELOG.md:76-86`：把 `130`/`143` 当作普通停止码的脚本应接受正常关闭的 `0`，这两个码现在表示操作者强制中止排空。Windows 契约与 Ctrl-C 等同 SIGINT。required CI 的 job 全部是 `ubuntu-latest`（`.github/workflows/ci.yml`），因此没有 Windows 行为覆盖；release 流水线在构建矩阵里声明 `x86_64-pc-windows-msvc` artifact，但不运行信号行为测试（`dist-workspace.toml:13-17`、`.github/workflows/release.yml`）。Windows 分支的类型检查与行为自动化仍需引入 Windows runner 后补充（会话记录曾用最小 Tokio crate 做过 `x86_64-pc-windows-gnu` 类型检查，当前树没有对应脚本可复核）。

## 指导

### 1. 只承诺可观测的顺序，不承诺信号计数

| 已发生的序列 | 可承诺结果 |
| --- | --- |
| 第一次 SIGINT/SIGTERM/Ctrl-C 被观测 | 停止接受新连接，排空在途请求，完成后退出 `0` |
| 第一次信号被观测后，第二次信号也再次被观测 | 放弃排空，立即退出；SIGINT/Ctrl-C 为 `130`，SIGTERM 为 `143` |
| 同一标准信号在第一次被观测前背靠背投递 | 可能合并；按第一次信号正常 drain 并退出 `0`，不保证出现 `130`（SIGINT）或 `143`（SIGTERM） |

“已经启动 graceful shutdown”在测试中可以用当前 stderr 诊断作为内部闸门，但诊断文案本身不是公开契约（`docs/contracts/cli.md:47`）。文档和 runbook 应写“第一次已启动 shutdown 后，第二个信号才保证强制退出”，不要写成“第二次信号总会退出 `130`/`143`”。

### 2. 继续把进程信号留在 CLI 边界

宿主信号、平台 Ctrl-C 和强制退出码属于 CLI；`server::run` 只接收一个 shutdown future 并负责连接排空（`src/main.rs:86-90`、`src/server.rs` 的 390 行起）。这不是形式划分：它让服务库不依赖操作系统信号 API，也允许测试或其他调用方提供不同的关闭触发器。新增“第几个信号”的逻辑不应漏进路由、请求处理或脚本层。

### 3. 保证路径等待状态，退化路径单独测试

测试强制退出时，先发送第一个信号，等待“shutdown 已启动”的可观测证据，确认进程仍在排空，再发送第二个信号并断言目标退出码。不要用固定 sleep 猜测第一次信号何时被处理。

背靠背投递是独立的退化契约：连续发送两个 SIGTERM 后，在有限期限内接受 `0` 或 `143`（当前回归只覆盖 SIGTERM；两个 SIGINT/Ctrl-C 的对应码是 `0` 或 `130`）。`0` 表示系统合并通知并完成正常 drain；`143` 表示第二次通知被观测并强制退出。两者都符合契约；进程必须在有限期限内以 `0` 或 `143` 退出，超时、其它退出码或信号终止都不符合契约。

当前测试工具已经体现了这两个层次：`wait_for_log` 轮询带超时，`assert_second_signal_forces_exit` 用 30 秒在途请求和保持的客户端连接建立排空场景，`back_to_back_sigterm_signals_still_exit` 则接受 `0 | 143`（`tests/cli.rs:3405-3418`、`tests/cli.rs:3484-3510`、`tests/cli.rs:3524-3545`）。

### 4. 不为不可兑现的保证增加复杂度

标准信号不排队是操作系统语义，不是 Tokio 或 axum 的配置遗漏。用 signal-hook、实时信号或额外计数层去“修复”背靠背合并，会让跨平台行为、依赖面和测试矩阵复杂化，却仍要重新定义公开契约。若未来确实要求每次投递都可计数，应把它作为独立的契约与设计问题处理，而不是悄悄扩大本次实现的保证。

## 为什么重要

1. **调用方的退出码判断必须稳定。** 正常操作停止是 `0`；`130`/`143` 只表示操作者放弃排空并强制退出。运维脚本、进程管理器和文档若把双击信号当作可靠的强制停止，会在信号合并时错误报告失败。
2. **契约必须停在平台能保证的边界。** 标准信号不排队，因此“第二次一定被观测”是不可实现承诺。明确合并语义能避免 future engineer 为追一个并不存在的实现 bug 反复改信号处理。
3. **固定 sleep 会制造假通过。** 它通常让第一次信号先被消费，于是测试只覆盖“已经启动 shutdown 后再发第二次”的保证路径，却漏掉真实的背靠背竞态；可观测状态和分开的退化测试才与契约一致。
4. **边界清晰能限制改动范围。** CLI 负责信号与强制退出，服务库只负责优雅排空；这样平台差异、退出码策略和排空机制不会互相渗透。

## 何时适用

- 修改 `serve` 的信号处理、退出码、平台分支或关闭诊断时。
- 编写或评审发送 SIGINT、SIGTERM 或 Windows Ctrl-C 的端到端测试时。
- 更新 CLI 契约、runbook、README 或部署脚本中“第二个信号会立即退出”的表述时。
- 为 Windows runner、未支持平台或新的进程管理器接入关闭行为时。
- 任何消费标准信号流、并假设每次投递都会被排队或单独观测的逻辑。

## 示例

### 保证路径：等第一次 shutdown 已启动，再发送第二次

```rust
send_signal(&fixture.child, "TERM");
wait_for_log(
    &fixture.log,
    "SIGTERM received; starting graceful shutdown",
);
assert!(fixture.child.try_wait().expect("try_wait").is_none());

send_signal(&fixture.child, "TERM");
let status = wait_for_exit(&mut fixture.child, Duration::from_secs(5));
assert_eq!(status.code(), Some(143));
```

这段模式来自 `assert_second_signal_forces_exit`（`tests/cli.rs:3484-3510`）。日志只用于内部同步，不是公开输出契约；关键是第二次信号发送前，已有证据表明第一次已经进入 shutdown，且进程尚未退出。

反模式是先把“背靠背”变成“有明显间隔”：

```rust
send_signal(&child, "TERM");
std::thread::sleep(Duration::from_millis(200));
send_signal(&child, "TERM");
assert_eq!(wait_for_exit(&mut child, Duration::from_secs(5)).code(), Some(143));
```

固定等待让第一次信号先被观测，测试实际覆盖的是保证路径，同时引入慢机器上的时序脆弱性。它不能证明两个信号背靠背时第二次一定可见。

### 退化路径：合并与强制退出都可接受

```rust
send_signal(&fixture.child, "TERM");
send_signal(&fixture.child, "TERM");
let status = wait_for_exit(&mut fixture.child, Duration::from_secs(5));
assert!(matches!(status.code(), Some(0 | 143)));
```

`0` 是第一次信号完成正常 drain，`143` 是第二次信号被观测后强制退出；测试只要求进程在期限内结束，避免把 OS 调度结果固化成错误断言（`tests/cli.rs:3524-3545`）。现有 Unix 回归还分别覆盖首次 SIGINT/SIGTERM 退出 `0`、SIGINT/SIGTERM 排空在途请求、第二次 SIGINT 退出 `130`、第二次 SIGTERM 退出 `143`（`tests/cli.rs:3420-3545`）。

## 相关

- CLI 契约：[`docs/contracts/cli.md`](../../contracts/cli.md)、[`docs/contracts/cli.zh-CN.md`](../../contracts/cli.zh-CN.md)
- 决策记录：[`plans/adr/0011-contract-compatibility.md`](../../../plans/adr/0011-contract-compatibility.md) 的 T10 修订
- 回归测试：`tests/cli.rs` 中 SIGINT/SIGTERM 首次信号、在途排空、第二次信号与背靠背用例
- issue：[#33](https://github.com/geeknonerd/stuntdouble/issues/33)

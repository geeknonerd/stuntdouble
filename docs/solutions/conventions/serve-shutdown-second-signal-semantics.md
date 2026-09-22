---
title: "A second shutdown signal is only guaranteed after the first one is observed"
date: 2026-09-22
last_updated: 2026-09-22
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
tags: [shutdown, signals, sigint, sigterm, ctrl-c, exit-codes, tokio, axum, end-to-end-tests, windows]
---

# Serve 的第二次关闭信号只在第一次被观测后才有保证

## 背景

T10（issue #33）为 `serve` 增加 graceful shutdown：首次 SIGINT/SIGTERM 停止接受新连接、排空在途请求并以 `0` 退出。信号处理留在 CLI 边界，`server::run` 只接收 shutdown future（`src/main.rs`、`src/server.rs`）。

最初契约写成“第二个信号总是立即终止”。代码评审用 30 秒在途请求做了背靠背两次 SIGTERM 的实测：stderr 只出现第一次信号，进程继续 drain。原因不是实现漏掉监听，而是标准（非实时）信号不排队：第一次通知尚未被消费时，第二次相同信号可能被 OS 与 Tokio 合并。测试里的固定 `sleep(200ms)` 恰好等待了第一次消费，掩盖了这个竞态。

## 指导

1. 契约只承诺可观测的行为：第二个信号在第一次信号已启动 graceful shutdown 后到达时，才保证放弃 drain 并按 `128+signal` 退出（SIGINT/Ctrl-C `130`、SIGTERM `143`）。
2. 背靠背信号合并时按第一次信号正常 drain 并退出 `0`；不要声称第二次信号一定会被观测，也不要为此引入 signal-hook、实时信号等新复杂度。
3. 测试等待可观测状态（例如 stderr 出现 `starting graceful shutdown`），不要用固定 sleep 建立时序。
4. 退化路径要有测试：背靠背信号允许 `0` 或 `143`，只要进程在期限内退出，证明合并不会造成永久挂起。
5. Windows Ctrl-C 有同类合并语义；当前 CI 只跑 Linux，Windows 只覆盖目标平台类型检查，行为自动化待 Windows runner 引入（ADR 0011 T10 修订）。

## 证据

- 契约与退出码：`docs/contracts/cli.md`、`docs/contracts/cli.zh-CN.md`
- 决策记录：`plans/adr/0011-contract-compatibility.md` 的 T10 修订
- 回归测试：`tests/cli.rs` 中 `sigint_drains_in_flight_requests`、`sigterm_drains_in_flight_requests`、`second_sigint_terminates_immediately_with_exit_code_130`、`second_sigterm_terminates_immediately_with_exit_code_143`、`back_to_back_sigterm_signals_still_exit`
- issue：#33

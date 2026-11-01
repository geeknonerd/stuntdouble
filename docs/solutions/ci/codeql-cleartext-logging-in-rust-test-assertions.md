---
title: CodeQL rust/cleartext-logging treats assert! panic messages as logging sinks
date: 2026-11-01
category: ci
module: CodeQL pull request merge protection
problem_type: integration_issue
component: ci
symptoms:
  - "CodeQL check fails with '4 new alerts including 4 high severity security vulnerabilities'"
  - "Alert rule is rust/cleartext-logging (CWE-312, CWE-359, CWE-532)"
  - "The reported location points to an assert! or assert_eq! macro in integration tests (e.g. tests/cli.rs)"
root_cause: tool_limitation
resolution_type: code_fix
severity: high
related_components: [testing]
tags: [codeql, code-scanning, rust, cleartext-logging, test-assertions, security-alerts, ci]
---

# CodeQL rust/cleartext-logging 将 assert! 的 panic 消息视为日志 sink

## 问题

在测试涉及认证、授权或敏感凭据（如 OAuth authorization code、client secret、Bearer token）的接口时，PR 的 CodeQL 安全扫描可能报告 High 级别的 `rust/cleartext-logging` 告警，从而阻断 GitHub ruleset 的合并门禁。

审查告警源码会发现，被举报的代码并不是生产日志（如 `log::info!` 或 `tracing::event!`），而是测试代码中的 `assert!` 或 `assert_eq!` 断言调用。

## 症状

- PR 上的所有编译与单元测试 CI job 均绿，但 `CodeQL` check-run 为 `conclusion: failure`。
- 检查 annotations 显示 `Cleartext logging of sensitive information`，并指出
  `This operation writes oauth_exchange(...) to a log file`。
- 告警代码行指向包含自定义 panic 消息的测试断言，例如：
  ```rust
  assert_eq!(response.status, status, "body: {}", response.body);
  ```
  或者：
  ```rust
  assert_eq!(userinfo_resp.status, 200, "body: {}", userinfo_resp.body);
  ```

## 没有奏效的做法

- **视作误报全部 dismiss**：在 Security 页面将每个告警标记为 `false positive` 虽可暂时绕过，但每次新增测试或修改断言都可能产生新告警；且 dismiss 历史会持续累积，增加代码审计负担。
- **重命名局部变量**：仅仅将测试中的 `token` 改为 `data` 或 `val` 并不能彻底规避污点追踪，因为上游请求参数（如包含 `client_secret` 的表单）已经被 CodeQL 内置的敏感数据模型标记。
- **在外部包装闭包**：将断言移至辅助函数并没有切断污点路径，CodeQL 会跨函数进行过程间污点追踪（inter-procedural taint tracking），并在辅助函数调用点报错。

## 解决方案

### 1. 切断断言中的污点格式化输出

CodeQL 之所以报告 `rust/cleartext-logging`，是因为 `assert_eq!(left, right, "body: {}", response.body)` 会在断言失败时使用 `format!` 构建 panic payload，并通过标准库的 panic hook 输出到 `stderr`。CodeQL 将输出到标准错误流的 panic 消息建模为了日志输出汇聚点（sink）。

修复方案是**移除断言失败提示中对整个敏感响应体或 headers 的直接插值**，恢复 Rust 标准断言形式：

```rust
// 修复前：插值响应体作为错误提示，触发 CodeQL rust/cleartext-logging
assert_eq!(response.status, status, "body: {}", response.body);
assert_eq!(
    response.header("content-type"),
    Some("application/json; charset=utf-8"),
    "headers: {:?}",
    response.headers
);

// 修复后：不把响应体注入 panic 格式化字符串
assert_eq!(response.status, status);
assert_eq!(
    response.header("content-type"),
    Some("application/json; charset=utf-8")
);
```

### 2. 精确断言结构化字段，避免打印原始载荷

如果测试需要断言错误响应的结构，使用提取后的枚举值或字段进行比对，而不是在断言失败时回显完整 body：

```rust
// 提取错误类型进行断言
assert_eq!(error_class(&response.body).as_deref(), Some(error));

// 验证敏感信息绝不外泄时，只断言不包含，消息保持简洁
for secret in [&token, &signature, &OAUTH_SIGNING_SECRET.to_string()] {
    assert!(!log.contains(secret), "credential leaked into logs");
}
```

### 3. 验证告警消除

修改后推送提交，触发 GitHub Actions 的 `analyze (rust)` 重新分析。检查 `CodeQL` check-run 是否直接变为 `SUCCESS`，且 `code-scanning/alerts?tool=CodeQL&ref=refs/pull/<N>/merge` 不再产生任何新的 `rust/cleartext-logging` 告警。

## 为什么有效

CodeQL 的 `rust/cleartext-logging`（CWE-312 / CWE-359 / CWE-532）规则定义如下数据流路径：

1. **Taint Source**：包含敏感语义命名的凭据源（如 `client_secret`、`password`、`token` 等）。
2. **Taint Propagator**：发起请求并返回的 `Response` 对象被标记为污点传播对象。
3. **Taint Sink**：标准库宏 `panic!`、`assert!`、`eprintln!`、`format!` 等格式化写入 `stderr` 的操作被视为日志输出汇聚点。

当断言宏写作 `assert_eq!(response.status, status)` 时，Rust 编译器生成的 panic 代码只对 `response.status`（非敏感的整型状态码）和期望状态码进行格式化，敏感污点源（`response.body`）完全未参与任何输出格式化操作，数据流中断，从而在根源上彻底消除了告警。

## 预防

- 在编写与敏感凭据（Token、密钥、Cookie 等）相关的测试断言时，避免使用带有敏感变量插值的自定义错误信息（如 `"body: {}", response.body`）。
- 状态码、头信息和错误代码的断言直接使用标准的 `assert_eq!(actual, expected)`。Rust 在断言失败时本身就会输出两端值。
- 当必须检查完整响应体时，通过精确反序列化（如 `serde_json::from_str`）核验具体字段的 key 与 value，避免将原始 body 字符串传入日志或 panic 格式化参数。

## 相关

- [docs/solutions/ci/codeql-pr-alerts-live-on-merge-ref.md](codeql-pr-alerts-live-on-merge-ref.md) — PR CodeQL 告警位于 merge ref 以及误报 dismiss 机制。
- [PR #216](https://github.com/geeknonerd/stuntdouble/pull/216) — 提供 OAuth userinfo 接口及其签名验证。
- [PR #221](https://github.com/geeknonerd/stuntdouble/pull/221) — 完整的 OAuth mock 验收测试套件，彻底消除 `rust/cleartext-logging` 告警。
- [docs/development.md](../../development.md) — 仓库的代码扫描与 CI 门禁规则。

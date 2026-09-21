---
title: Bundled webpki roots fall outside the MIT/Apache license allowlist
date: 2026-09-21
category: dependencies
module: upstream HTTP TLS trust
problem_type: license_policy_conflict
component: upstream
symptoms:
  - "cargo deny check licenses failed on webpki-roots 1.0.9 with CDLA-Permissive-2.0"
  - "The HTTPS client compiled, but the repository license gate rejected the bundled root store"
root_cause: bundled_root_store_license
resolution_type: dependency_swap
severity: medium
tags: [tls, cargo-deny, licenses, ureq, rustls]
---

# 打包的 webpki 根证书不在 MIT/Apache 许可白名单内

## 问题

`ctx.http.get` 需要 HTTPS 支持，最初配置的 `ureq` 使用了默认的 `rustls` feature。该 feature 会引入 `webpki-roots` 1.0.9，其数据以 CDLA-Permissive-2.0 许可。仓库的依赖政策只允许 MIT OR Apache-2.0，因此即使 Rust 代码本身是宽松许可，`cargo deny check licenses` 仍然拒绝该 crate。

## 试过但无效的做法

- **把 CDLA-Permissive-2.0 加进白名单**：`AGENTS.md` 要求新增依赖必须是 MIT OR Apache-2.0，为一份打包数据放宽白名单等于悄悄削弱这条规则。
- **保留 `ureq` 默认的 `rustls` feature**：TLS 栈其余部分没问题，但打包的根证书库始终留在依赖图里。

## 解决方案

改用 `ureq` 的 `rustls-no-provider` 加 `rustls-platform-verifier`，并显式提供 ring crypto provider：

- `ureq = { default-features = false, features = ["rustls-no-provider", "platform-verifier"] }`
- `rustls = { default-features = false, features = ["ring"] }`
- agent 的 `TlsConfig` 设为 `RootCerts::PlatformVerifier` 与 ring provider。

TLS 信任改为来自宿主证书库，因此操作系统里安装的企业根证书同样可用。`rustls-platform-verifier` 仍会为 wasm/android target 提到 `webpki-root-certs`，所以 `deny.toml` 通过 `[graph] targets` 把 `cargo-deny` 限定在发布平台（Linux x86_64、macOS arm64、Windows x86_64），`docs/development.md` 记录了该范围。

## 验证

- `cargo deny check` 在 licenses、sources、bans、advisories 上全部通过。
- `cargo audit` 对锁定依赖图没有报告任何公告。
- `ctx.http.get` 的端到端测试覆盖 2xx、4xx/5xx、重定向、超时与传输层失败。

## 取舍

- TLS 证书信任交给操作系统，而不是打包 Mozilla 根证书集。对本地 mock server 来说这正是期望行为，也让企业 MITM 根证书可用。
- 依赖检查覆盖项目实际发布的平台；不受支持平台的平台专属依赖不在门禁范围内。

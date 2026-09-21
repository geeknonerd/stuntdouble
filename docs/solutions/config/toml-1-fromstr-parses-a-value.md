---
title: toml 1.x `FromStr` parses a value, not a document
date: 2026-09-20
category: config
module: TOML configuration parsing
problem_type: api_behavior_change
component: config
symptoms:
  - "After upgrading to toml 1.x every configuration failed with `TOML parse error at line 1, column 15 / unexpected content, expected nothing`"
  - "The same fixture parsed fine with toml 0.8, and the error pointed at the first `key = value` pair, not at a syntax mistake"
root_cause: api_semantics_change
resolution_type: code_fix
severity: high
tags: [toml, dependency-upgrade, parsing, config]
---

# toml 1.x 的 `FromStr` 只解析单个值，不解析整个文档

## 问题

`toml` 从 0.8 升到 1.x 后，`stuntdouble validate` 拒绝所有配置，连仓库自带的 fixture 也不例外。解析器报：

```text
TOML parse error at line 1, column 15
  |
1 | config_version = "1"
  |               ^
unexpected content, expected nothing
```

这条错误具有误导性：输入本身是合法 TOML。问题出在加载器使用了 `text.parse::<toml::Value>()`。

## 试过但无效的做法

- **在 1.x 内降级**：`toml 1.0.7` 同样复现失败，说明这不是某个 patch 版本的回归。
- **为了保留 toml 0.8 而压低 MSRV 栈**：Boa 0.20 / 0.21 会引入已归档的 `paste` crate，并需要受 RUSTSEC-2026-0009 影响的 `time` 版本，因此 `cargo deny check advisories` 失败。必须在代码里修，而不是钉死旧依赖。

## 解决方案

用 `toml::from_str` 解析文档，它在 0.8 与 1.x 都是文档级入口：

```rust
// Before: parses a single inline value as of toml 1.x
let root: toml::Value = text.parse().map_err(/* ... */)?;

// After: parses the whole document
let root: toml::Value = toml::from_str(&text).map_err(/* ... */)?;
```

`src/config.rs` 里留了一条注释指向这一差异，避免下一位读者重新踩一遍。

## 为什么这样可行

在 `toml` 1.x 中，`impl FromStr for Value` 委托给 `ValueDeserializer::parse`，那是**内联值**解析器，因此 `config_version = "1"` 这样的文档在第一个 token 之后就失败。`toml::from_str::<toml::Value>` 使用文档反序列化器，返回预期的 table。`toml 0.8` 恰好两者都接受，所以这次升级把潜藏的误用暴露了出来。

## 预防

- 把 `str::parse::<toml::Value>()` 视为「解析单个值」；输入是配置文档时一律用 `toml::from_str`。
- 至少保留一个针对真实文件的端到端 `validate --config` 测试，让解析器语义变化在测试中大声失败，而不是只在单元 fixture 里悄悄通过。
- 依赖大版本升级后，先跑全部门禁（`test`、`clippy`、`deny`）再动 MSRV 数字；第一个失败可能是语义变化而非版本约束。

## 相关

- `Cargo.toml` 现在依赖 `toml = "1"`。
- Boa 0.20、0.21 与 0.22 之间的 MSRV／安全取舍记录在 [ADR 0003](../../../plans/adr/0003-script-first-multi-runtime.md) 与 `docs/development.md`。

---
title: "A prerelease tag is invisible to release-plz's release regex"
date: 2026-09-24
category: ci
module: release-plz tag detection
problem_type: integration_issue
component: ci
symptoms:
  - "`Release-plz` 在每次 push 到 `main` 后都 success，但从不打开版本 PR"
  - "日志出现 `No release tag found matching pattern ^v(\\d+\\.\\d+\\.\\d+)$` 与 `the repository is already up-to-date`，随后 `release_pr_output: {\"prs\":[]}`"
  - "`CHANGELOG.md` 的 `## [Unreleased]` 持续累积，仓库只有一个 tag `v0.1.0-alpha.1`"
root_cause: logic_error
resolution_type: workflow_improvement
severity: high
tags: [release-plz, prerelease, git-tags, release-automation, silent-failure, conventional-commits]
---

# prerelease tag 对 release-plz 的发布正则不可见

## 问题

`release-plz` 在每次 push 到 `main` 后运行并报 success，却永远不打开版本 PR：#52、#53、#54、#56 四条改动堆在 `## [Unreleased]` 里，仓库始终只有 `v0.1.0-alpha.1` 一个 tag。整条发布链看起来是绿的，实际是静默停摆。

## 症状

`Release-plz` 运行中 `release-plz PR` job 的输出：

```
INFO No release tag found matching pattern `^v(\d+\.\d+\.\d+)$`. Package stuntdouble will be treated as initial release.
INFO determining next version for stuntdouble 0.1.0-alpha.1
INFO stuntdouble: next version is 0.1.0-alpha.1
INFO the repository is already up-to-date
release_pr_output: {"prs":[]}
```

判定为「首次发布」后，它提出的 next version 就是 `Cargo.toml` 里的当前版本，于是没有可提交的改动、没有 PR；`release` job 也因为没有新版本而什么也不建。

## 不可行的做法

- **调 `git_tag_name` 模板**：把模板写成 `v{{version}}` 后重跑，生成的正则不变——上游把版本占位符替换成硬编码的数字组，与模板无关（本地实验确认）。
- **手工 bump `Cargo.toml`**：首次发布路径永远以清单版本作为 next version，bump 之后仍然读出「已是最新」，依旧没有 PR。
- **依赖预编译二进制做完整回归**：本地 `release-plz 0.3.169` 在仓库出现第二个 tag 后段错误（exit 139），无法用它验证修复；结论改由上游源码得出（见下）。

## 解决方案

**版本与 tag 一律使用纯 `vMAJOR.MINOR.PATCH`，不带 `-alpha`/`-beta`/`-rc` 后缀。** pre-1.0 的不稳定语义由 `0.x`、CHANGELOG 文字与 release notes 表达。

`v0.1.0-alpha.1` 对 release-plz 不可见，所以 `0.2.0` 是一次性桥接：版本号与 `CHANGELOG.md` 段由人工在同一个 PR 写好（PR #62），合并后 `release-plz release` 立即建出 tag `v0.2.0`、派发 cargo-dist 产物流水线并发布 Release（含三平台归档、校验和、CycloneDX SBOM、`ctx-api-v1.d.ts`、GHCR digest）。此后 tag 形如 `v0.2.0` 能被正常识别，版本 PR 恢复由 release-plz 打开。

规则落在 `docs/development.md` 的「版本号」与「发布流程」，约束注释落在 `release-plz.toml` 的 `git_only` 旁。代价要一并记住：cargo-dist 依据版本后缀决定是否把 GitHub Release 标为 pre-release，纯数字版本因此不会被自动标记，需要时得单独在 `release-extras` 补一步。

## 为什么有效

上游 `release-plz`（tag `release-plz-v0.3.169`）的 `crates/release_plz_core/src/release_regex.rs::get_release_regex` 把渲染后的 tag 模板做正则转义，然后把版本占位符替换成**固定形态**的捕获组，再整体锚定：

```rust
let pattern = escaped.replace(&regex::escape(VERSION_PLACEHOLDER), r"(\d+\.\d+\.\d+)");
let full_regex = format!(r"^{pattern}$");
```

`^v(\d+\.\d+\.\d+)$` 能匹配 `v0.2.0`，永远不匹配 `v0.1.0-alpha.1`。只有先识别出「上一次发布」，release-plz 才会基于 Conventional Commits 计算下一版本并打开 PR——所以问题不在版本策略的偏好，而在探测本身。

## 预防

- 不要用 prerelease tag 作为发布标记；`rc`/`beta` 同理（同一个正则）。
- 发布 PR 迟迟不出现时，先看 `release-plz` 日志里的 `No release tag found matching pattern` 与 `already up-to-date` 两行——它们就是这条缺陷的签名。
- 本地复现：取对应版本的 release-plz 二进制，克隆仓库（含 tag），在副本里跑 `release-plz update`，对比上面四行输出；改动修复后再跑一次看 next version 是否前移。
- 验收证据是「合并后出现发布 PR / 新 tag 与 Release」，不是 workflow 变绿。

## 相关

- `docs/development.md` —— 版本号与发布流程的权威规则（含本次的纯数字版本约束）
- `release-plz.toml` —— 约束注释与 `git_only`、`git_tag_enable` 配置
- issue #61、PR #62 —— 本问题的定位与修复
- `docs/solutions/ci/release-plz-pat-required-to-trigger-pr-workflows.md` —— 同一条发布链上的另一个静默失效（token 触发的 workflow 缺失）

---
title: "Slice status lives in several surfaces that the docs gate does not check"
date: 2026-09-23
category: conventions
module: slice status documentation
problem_type: convention
component: documentation
severity: medium
applies_when:
  - "落地任何会被状态段落提到的切片、能力或 demo 变化时"
  - "issue 只点名了其中一个状态版面（例如只要求改根 README）时"
  - "刷新 learning 文档或使用指南里描述当前树状态的段落时"
  - "评审分支、判断文档是否完整时"
related_components: [documentation, ci]
root_cause: missing_workflow_step
resolution_type: documentation_update
tags: [status-drift, documentation, slice-status, changelog, agents-md, bilingual-docs, docs-links, review-checklist]
---

# 切片状态分散在多个版面，docs-links 只保证链接与双语配对

## 背景

T13（issue #54，分支 `feat/offline-document-demo`；截至本文写作时尚未推送、未开 PR）为 `demo/` 增加离线路由。issue 的文档工作只点名了 demo README 与根 `README.md`；照此落地后，分支 review 的 Standards 轴发现三处状态版面仍停在上一切片：`docs/index.md` 与 `docs/index.zh-CN.md` 写 T1–T12、`CONTRIBUTING.md` 的项目状态写 “through T12”、`CHANGELOG.md` 的 `## [Unreleased]` 没有本次条目。随后的文档同步又发现 `docs/solutions/conventions/script-owned-upstream-error-mapping.md` 里描述 demo 夹具现状的段落已经过时——它仍把 `demo/files/.gitkeep` 说成该文件根之所以存在的原因。

这些漏更都不是搞错了规则，而是没有一处文档列出 “状态都写在哪里”。`docs/development.md` 规定了文档语言分层、公开契约与 CHANGELOG 的同步义务，但没有点名状态版面清单。

## 指南

落地切片时按下面这张表人工扫一遍（本节路径均相对仓库根）：

| 版面 | 写什么 | 先例 |
| --- | --- | --- |
| `README.md` + `README.zh-CN.md` 的 Status / 当前状态段 | 切片叙事：T 编号与能力变化 | PR #55、#57 各自更新 |
| `docs/index.md` + `docs/index.zh-CN.md` 的状态段 | 同一叙事（GitHub Pages 首页） | PR #55、#57 各自更新 |
| `CONTRIBUTING.md` 的 Project status | 同一叙事 | PR #55、#57 各自更新 |
| `CHANGELOG.md` 的 `## [Unreleased]` → `### Added` | 每条切片一条英文 bullet，带 issue 链接 | PR #55 加了 #52 条目，PR #57 加了 #53 条目 |
| `AGENTS.md` | 不写切片状态；只保留会改变 agent 行为的持久规则，并把状态指向 `README.md`、`CONTRIBUTING.md` 与 `CHANGELOG.md` | n/a（不再是状态版面；见下方修订） |
| 描述该区域现状的 learning 文档与指南 | 只改与本次变化冲突的段落 | T13 同步了 demo 夹具段落与 `docs/guide/getting-started.md` 的「下一步」 |

最省事的机械做法：开 PR 前用上一个切片号与对应 issue/PR 号（例如 `T12`、`#57`）全仓 grep 一次，对命中处逐条判断它是「历史记录」还是「现状描述」。

## 为什么重要

`docs-links` 门槛只做两件事：`lychee --offline` 检查全部 Markdown 的本地链接，以及校验每个 `.zh-CN.md` 都有同名英文文件、两页都含语言切换链接（`.github/workflows/ci.yml` 的 `docs-links` job）。`docs/development.md` 明确写着 “外链与译文漂移不做自动检查”。因此状态句子过期、漏掉一个版面、中文译本内容没跟上，全部门禁照常通过——只有人工或 agent review 才会发现，而能否发现取决于 review 是否恰好扫到那一处。

状态版面还是新 agent 的第一入口：`README.md`、`CONTRIBUTING.md` 与 `CHANGELOG.md` 决定了后来者认为 “什么已实现”，读错会让后续工作建立在不存在的现状上。`AGENTS.md` 不复制这些状态，只指向它们，维护规则见 `~/.codex/AGENTS-OPTIMIZATION.md`。

## 何时适用

- 落地任何会被状态段落提到的切片、能力或 demo 变化时。
- issue 只点名了其中一个状态版面时：点名是下限，不是免检名单。
- 刷新 learning 文档或使用指南中描述当前树状态的段落时。
- 评审分支时：把 “状态版面是否同步” 当作独立检查项，而不是期待读者偶然撞见。

## 示例

T13 的实际证据：

- 漏更（同步前）：`README.md` 已宣告 T13，而 `docs/index.md`、`docs/index.zh-CN.md`、`CONTRIBUTING.md` 仍停在 T1–T12，`CHANGELOG.md` 的 `## [Unreleased]` 缺本次条目；按当时的约定，修复把四处与 `AGENTS.md` 一起同步。
- 过时现状段落：`docs/solutions/conventions/script-owned-upstream-error-mapping.md` 的 demo 段落改为 “T5 时该目录只靠 `.gitkeep` 存在，T13 起目录内是真实夹具（`metadata.json` 与两张公开 PDF）”，并补上离线路由不读 `METADATA_API_URL`。
- 不要动的历史记录：`CHANGELOG.md` 已发布的 `0.1.0-alpha.1` 小节、`docs/solutions/` 中以会话日期叙述的探查过程、`docs/development.md` 里针对当时工具链的版本说明——它们是历史，不是现状描述。

## 修订（2026-09-26）：AGENTS.md 不再是状态版面

`AGENTS.md` 的维护遵循 `~/.codex/AGENTS-OPTIMIZATION.md`：只保留会改变 agent 行为的持久规则，不承载切片状态、版本号或环境状态。agent 需要现状时，从 `README.md`、`CONTRIBUTING.md` 与 `CHANGELOG.md` 读取，而不是从 `AGENTS.md` 复制。

PR #55/#57 同步 `AGENTS.md` 项目快照是当时的做法，保留为历史例证，不再作为当前清单要求。若发现 `AGENTS.md` 仍含过期状态，应另开一次 AGENTS.md 优化，按全局指南删除或改为指向现状文档，不在切片文档同步 PR 中顺手重写。

## 相关

- `docs/development.md` —— 文档语言分层、本地检查与 docs-links 门槛的权威说明
- `.github/workflows/ci.yml` —— `docs-links` job 的实际检查范围
- `AGENTS.md` —— 持久行为规则与「先读」清单；切片状态不在此维护
- `CONTRIBUTING.md` —— 面向贡献者的项目状态入口

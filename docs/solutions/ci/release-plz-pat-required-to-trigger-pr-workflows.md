---
title: Release PRs created with GITHUB_TOKEN do not trigger pr-title
date: 2026-09-22
category: ci
module: release-plz GitHub Actions workflow
problem_type: integration_issue
component: ci
symptoms:
  - "Before the body edit, PR #47 had no pr-title check/run and therefore could not satisfy branch protection (the Expected UI state was observed in the session)"
  - "The first and only pr-title run, 35734352094, was created after the body edit"
  - "CI, DCO, and CodeQL runs were created in approval-required state; pr-title had no run to approve or rerun"
root_cause: incomplete_setup
resolution_type: config_change
severity: high
tags: [github-actions, release-plz, github-token, fine-grained-pat, pull-request-target, required-checks, event-suppression, branch-protection, ci]
---

# Release PRs created with GITHUB_TOKEN do not trigger pr-title

## 问题

`release-plz` 在 `main` 收到 push 后通过 `release-pr` 创建或更新发布 PR（`.github/workflows/release-plz.yml:47-69`），而仓库把 `pr-title` 列为合并前必须通过的检查（`docs/development.md:20`）。当工作流只能使用仓库 `GITHUB_TOKEN` 创建 PR 时，PR 可以成功创建，其他 `pull_request` workflows 会以 approval-required 状态产生 run，但 `pr-title` 使用的 `pull_request_target` 不会产生 run；会话中观察到发布 PR #47 因此无法满足分支保护条件。

## 症状

- 会话中观察到发布 PR #47（`chore: release v0.1.0-alpha.1`）的必需检查显示 `pr-title Expected — Waiting for status to be reported`，无法满足分支保护条件。历史 UI 状态不可回放；API 可复核的是编辑前没有 `pr-title` run，最早且唯一的 run 是编辑后的 `35734352094`。
- 编辑前 PR #47 没有 `pr-title` check/run；`gh pr checks <PR>` 与 `gh run list --workflow pr-title.yml` 是复核这两个列表的诊断命令，问题不是 job 排队或执行失败。
- 本次事件中 CI、DCO 与 CodeQL 的 runs 以 approval-required 状态创建、稍后启动；`pr-title` 没有可供批准或重跑的 run。

## 没有奏效的做法

- 继续等待 `pr-title` 自动出现：`pr-title` 监听 `pull_request_target` 的 `opened`、`edited`、`synchronize`、`reopened`（`.github/workflows/pr-title.yml:3-5`），但由 `GITHUB_TOKEN` 创建 PR 产生的 `pull_request` runs 进入 approval-required 状态，`pull_request_target` 的 `pr-title` 没有生成 run。
- 批准或重跑现有 run：批准操作可以启动本仓库其他需要批准的 run，但 `pr-title` 根本没有 run，因而没有可操作对象。
- 仅依赖 `GITHUB_TOKEN` 回退：`release-plz-pr` job 已有 `contents: write` 与 `pull-requests: write`（`.github/workflows/release-plz.yml:51-53`），足以创建 PR，但增加这些权限不会改变 `GITHUB_TOKEN` 事件被抑制的事实。
- 关闭再打开 PR：更早的 PR #22 曾通过 close/reopen 恢复 `pull_request` workflow（session history）；PR #23 没有使用这条路径。它是操作性兜底，但会话记录未识别 PR 创建者或 token 差异，不能解释本次 `pr-title` 完全没有 run 的原因，也不能防止复发。

## 解决方案

已经卡住的 PR #47 通过一次 PR 正文编辑产生的 `edited` 事件恢复；例如：

```bash
gh pr edit 47 --body-file <body-file>
```

GitHub 保留的证据是 `edited` 事件后出现并通过的 `pr-title` run `35734352094`，PR #47 随后完成 squash merge。具体本地命令无法由 API 回放，因此把它视为可复现的操作建议；该路径只用于解除已卡住的 PR，不是长期修复。

长期修复是为 release-plz 配置细粒度 PAT `RELEASE_PLZ_TOKEN`，并让工作流优先使用它。当前定义已经存在：

```yaml
env:
  GITHUB_TOKEN: ${{ secrets.RELEASE_PLZ_TOKEN || secrets.GITHUB_TOKEN }}
```

`.github/workflows/release-plz.yml:34` 与 `.github/workflows/release-plz.yml:70` 分别在 release 与 release-pr job 中读取该表达式。PAT 的权限要求是 `Contents: Read and write` 与 `Pull requests: Read and write`（`docs/development.md:185`）；ADR 0012 也把 tag、分支与 release PR 使用该 PAT 记为 T9 约束（`plans/adr/0012-release-artifacts-and-supply-chain.md:66`）。当前表达式只在 `RELEASE_PLZ_TOKEN` 未配置或为空时回退到 `GITHUB_TOKEN`；非空但失效的 token 仍会被选中，release-plz 会直接失败，不会自动回退。`docs/development.md:186` 的“失效即回退”表述不准确，需要单独修正。

验证不能只检查 secret 名称。验证 run `35736475288`（临时工作流，`push` 事件）使用 `RELEASE_PLZ_TOKEN` 创建了 PR #48，并观察到四个独立的 `pull_request` runs 自动排队：CI `35736502953`、DCO `35736502982`、CodeQL `35736503022`、Release `35736503330`，另有 `pull_request_target` run `35736503260`；随后 PR #48 已关闭、临时 refs 与验证分支均已清理，该临时工作流不保留在当前树中。更新 PAT 后应以同样方式验证，要求写入 `docs/development.md:187`。

## 为什么有效

GitHub 的自动令牌文档规定：使用仓库 `GITHUB_TOKEN` 执行任务时，由其触发的事件通常不会创建新的 workflow run，`workflow_dispatch` 与 `repository_dispatch` 例外；对于 `GITHUB_TOKEN` 创建 PR 产生的 `pull_request` `opened`、`synchronize`、`reopened`，workflow run 会进入 approval-required 状态。参见 [Trigger a workflow](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/trigger-a-workflow#triggering-a-workflow-from-a-workflow)。本次事件中其他 `pull_request` runs 以 approval-required 状态创建、稍后启动，而 `pr-title` 使用的 `pull_request_target` 没有产生 run；诊断时应区分“等待批准的 run”和“根本没有 run”。

PAT 是独立于自动 `GITHUB_TOKEN` 的凭据，不受该抑制规则约束。当前表达式在 `RELEASE_PLZ_TOKEN` 可用时不会再回退到 `GITHUB_TOKEN`（`.github/workflows/release-plz.yml:70`）；PAT 创建 release PR 后，仓库订阅的 `opened` activity type 会触发 `pull_request_target`，由名为 `pr-title` 的 job 上报必需检查（`.github/workflows/pr-title.yml:4-5 和 .github/workflows/pr-title.yml:13-14`）。

对已经存在的 PR，真实正文编辑会发送 `edited` activity type；它正是该工作流订阅的四种活动之一（`.github/workflows/pr-title.yml:4-5`）。`pull_request_target` 支持的活动类型与安全语义见 [Events that trigger workflows](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#pull_request_target)。

## 预防

- 把 `RELEASE_PLZ_TOKEN` 当作发布自动化的前置条件，并按最小权限配置为 Contents 与 Pull requests 读写（`docs/development.md:185`、`plans/adr/0012-release-artifacts-and-supply-chain.md:66`）。触发 `release.yml` 仍使用 release job 内 `GITHUB_TOKEN` 的 `actions: write`（`.github/workflows/release-plz.yml:14-16 和 .github/workflows/release-plz.yml:35-44`），不需要给 PAT 增加 Actions 权限。
- PAT 更新、轮换或失效处理后，必须用一个由该 PAT 创建的测试 PR 验证 `pull_request` workflows 自动排队，不能只检查 secret 名称（`docs/development.md:187`）。
- required check 长时间停在 `Expected` 时，同时检查 `gh pr checks <PR>` 与 `gh run list --workflow <name>`。如果 check 列表和 run 列表都没有对应项，应按“事件未触发”排查，而不是等待或批准一个不存在的 job。
- 若既有 PR 已因缺少事件而卡住，使用真实正文编辑触发 `edited` 事件，例如 `gh pr edit <PR> --body-file <body-file>`；不要为了绕过问题删除 `pr-title` 必需检查（`docs/development.md:20`）。
- 关闭再打开 PR 只能作为最后手段；使用后要记录 PR 创建者与 token 来源，避免再次把事件触发差异误判为仓库偶发问题（session history：PR #22 使用过该兜底，PR #23 没有）。

## 相关 issue

- PR #47：受影响的 release PR，通过正文编辑触发 `pr-title` 后已 squash merge。
- PR #48：使用 PAT 创建临时 PR 的验证，验证后已关闭；对应验证 run 为 `35736475288`。
- PR #49：补充 `RELEASE_PLZ_TOKEN` 权限、回退行为与验证要求的文档变更，已合并。
- PR #22：更早通过关闭/重开恢复 PR workflow 的会话记录；PR #23 没有使用该方式。会话记录未提到 token 触发差异（session history）。
- `docs/development.md:186`：当前“失效即回退”的表述与 `||` 表达式语义不符，需要单独修正；本次 ce-compound 不直接修改该仓库文档。
- 相关仓库定义：`.github/workflows/release-plz.yml`、`.github/workflows/pr-title.yml`、`docs/development.md:176-187`、`plans/adr/0012-release-artifacts-and-supply-chain.md:58-66`。
- 相关平台文档：[Trigger a workflow](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/trigger-a-workflow#triggering-a-workflow-from-a-workflow)、[Events that trigger workflows](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#pull_request_target)。

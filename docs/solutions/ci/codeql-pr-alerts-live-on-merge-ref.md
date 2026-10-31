---
title: PR CodeQL alerts live on the merge ref, and dismissing them does not rerun the scan
date: 2026-10-31
category: ci
module: CodeQL pull request merge protection
problem_type: integration_issue
component: ci
symptoms:
  - "The PR's CodeQL check reports: 8 new alerts including 1 critical severity security vulnerability"
  - "`code-scanning/alerts?state=open` returns zero, so the PR looks clean even though the check is red"
  - "After every alert is dismissed, the same CodeQL check stays FAILURE until a new analysis runs"
root_cause: incomplete_setup
resolution_type: workflow_improvement
severity: high
related_components: [testing, release]
tags: [codeql, code-scanning, pull-request, merge-ref, false-positive, github-actions, branch-protection, ci]
---

# PR 的 CodeQL 告警位于 merge ref，dismiss 不会重新运行扫描

## 问题

一个 pull request 没有引入真实漏洞，但它的 `CodeQL` 检查阻止了合并。直接查询
open alerts 却显示仓库没有告警，因此常见的「dismiss 误报」建议既不能解释检查
为何失败，也不能说明如何让它转绿。

解决这个问题依赖两个不在 workflow 文件里的事实：PR 分析会把告警上传到
`refs/pull/<N>/merge`，而 dismiss 只改变告警状态，不会触发新分析。因此失败的
CodeQL 检查会一直保持失败，直到另一次扫描运行。

## 症状

- `gh pr view <N> --json statusCheckRollup` 显示所有 required CI job 均为绿色，
  另有一个 `CodeQL` 检查为 `conclusion: FAILURE`；它通常在 `analyze (rust)`
  job 成功后几秒出现。
- 该检查的输出给出告警数量，例如
  `8 new alerts including 1 critical severity security vulnerability`，而
  `gh api "/repos/{owner}/{repo}/code-scanning/alerts?state=open"` 返回 `[]`。
- 在 Security 页面 dismiss 后，每个告警都变为 `dismissed`，但 PR 检查仍保留
  旧的 `FAILURE` 结论；只有后续分析才会把它替换为 `SUCCESS`。

## 没有奏效的做法

- 只读取默认分支的告警列表。API 查询不带 `ref` 时不会返回 PR merge ref 上的
  告警，所以列表为空而 PR 检查仍为红色。
- 把 `analyze (rust)` 当作 PR 检查。workflow job 已成功；失败的检查由 GitHub
  Advanced Security 根据上传的分析结果创建，其 annotations 才列出每个告警。
- dismiss 告警后持续轮询。没有新扫描入队，检查不会变化；等待不是修复。

## 解决方案

1. 先读取失败检查的 annotations。它们包含每个告警的 rule id、文件、行号与
   严重级别：

   ```bash
   gh pr view <N> --json statusCheckRollup \
     --jq '.statusCheckRollup[] | select(.name=="CodeQL") | {name,conclusion,detailsUrl}'
   gh api "/repos/{owner}/{repo}/check-runs/<check-run-id>/annotations" \
     --jq '.[] | {path,start_line,title,annotation_level}'
   ```

2. 按 PR merge ref 查询告警，而不是查询仓库默认分支：

   ```bash
   gh api "/repos/{owner}/{repo}/code-scanning/alerts?tool=CodeQL&ref=refs/pull/<N>/merge" \
     --jq '.[] | {number,state,rule:.rule.id,path:.most_recent_instance.location.path,line:.most_recent_instance.location.start_line}'
   ```

3. 根据证据逐个判断告警。测试向量、固定的 fixture 凭据、HMAC 校验，以及断言
   敏感信息未出现在日志中的测试，都是常见误报。dismiss 时选择最接近的原因：

   ```bash
   gh api -X PATCH "/repos/{owner}/{repo}/code-scanning/alerts/<number>" \
     -f state=dismissed -f dismissed_reason="false positive"
   ```

   API 的 `dismissed_reason` 是固定枚举（`false positive`、`won't fix`、
   `used in tests`、`mitigated`）；`mitigated` 用于告警本身真实、但已在别处
   缓解的情况。需要详细解释时，在 PR 或 issue 中留言，不要向该字段写入自由
   文本。

4. 触发一次新分析。dismiss 只改变告警状态，不会改变已完成的检查。推送一个带
   DCO 签名的空提交，是重新运行 `pull_request` workflow 的最小侵入方式：

   ```bash
   git commit --allow-empty -s -m "chore(ci): rerun CodeQL after dismissing false positives"
   git push
   ```

5. 合并前同时确认两层结果：新的 `CodeQL` 检查为 `SUCCESS`，且 merge-ref 告警
   查询不再返回 `open` 条目。

## 为什么有效

代码扫描按 analysis ref 存储结果。PR workflow 上传到 merge ref，因此 PR 的告警
不属于默认分支的告警集合；不带 `ref` 的查询静默回答了另一个问题。`CodeQL`
检查是一次已完成分析的快照：dismiss 告警会改变告警状态，但不会把分析任务
入队，所以旧结论会一直保留到新扫描上传结果。重新运行 workflow 既会重新评估
当前树，也会给检查一个新结论。

本仓库的 ruleset 配置为 `alerts_threshold = none` 与
`security_alerts_threshold = high_or_higher`，因此即使 CodeQL 不是 required
status check，merge ref 上一个 high 或 critical 的 open 告警仍会阻止合并
（[docs/development.md](../../development.md) 的「Pull request」一节）。
dismiss 已确认的误报是受支持的路径；停用 ruleset 不是。

## 预防

- CodeQL 检查失败时，从它的 annotations 和
  `ref=refs/pull/<N>/merge` 查询开始排查；不要根据未过滤的列表得出「没有
  告警」的结论。
- 把仅用于测试的凭据和已知答案向量明确限制在测试范围内。如果规则仍然触发，
  用最接近的枚举原因 dismiss，并在 PR 中记录依据，避免下一位 reviewer 重复
  判断。
- 每次 dismiss 后都推送新提交或重新运行 workflow，然后同时确认检查与新
  merge-ref 告警查询。
- 不要把 CodeQL 加入 required status checks；现有 ruleset 才是合并门禁
  （[docs/development.md](../../development.md)）。

## 相关

- [docs/development.md](../../development.md) — CodeQL merge-protection ruleset
  与误报 dismiss 策略。
- [.github/workflows/codeql.yml](../../../.github/workflows/codeql.yml) — PR、
  push 与定时运行的 CodeQL 分析。
- [node20-deprecation-annotations-only-cover-node20-actions.md](node20-deprecation-annotations-only-cover-node20-actions.md)
  — 另一种 check-run annotation 排查，对应不同的 CI 告警类型。
- [Issue #45](https://github.com/geeknonerd/stuntdouble/issues/45) — 选择 code
  scanning merge protection 而不是 required check 的决策。
- [PR #212](https://github.com/geeknonerd/stuntdouble/pull/212) — 触发本次
  CodeQL merge protection 的 OAuth code exchange 变更。

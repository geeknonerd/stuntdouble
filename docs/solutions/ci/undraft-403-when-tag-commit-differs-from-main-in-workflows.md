---
title: "Tag 提交与默认分支的 workflow 文件不一致会让 cargo-dist announce 的 undraft 收到 403"
date: 2026-10-12
category: ci
module: cargo-dist release announce
problem_type: integration_issue
component: ci
symptoms:
  - "announce job 的 Create GitHub Release 步骤报 `HTTP 403: Resource not accessible by integration`,URL 是 releases/<id> 的 PATCH 端点(即 gh release edit --draft=false 的调用)"
  - "同一 bash 步中 gh release upload 已成功、draft 上资产齐全,只有 undraft 被拒绝;Release 停留在未公开的 draft"
  - "run 38135855259(v1.0.2)的 announce job 于 2026-10-11T11:43:00Z 启动、11:43:07Z 失败,release-extras 此前已全部通过"
root_cause: missing_permission
resolution_type: documentation_update
severity: high
retire_when: "上游 axodotdev/cargo-dist#2415 关闭并确认修复(announce 步骤对与默认分支存在 workflow 文件差异的 target 不再 403),或 GitHub 放宽 release PATCH 的 workflow 修改授权要求;检查该 issue 状态、dist generate 后的 announce 步骤与 GitHub REST 'Update a release' 描述"
tags: [github-actions, cargo-dist, github-token, workflow-scope, release-recovery, draft-release, undraft, 403]
---

# Tag 提交与默认分支的 workflow 文件不一致会让 cargo-dist announce 的 undraft 收到 403

## 问题

2026-10-11 的 v1.0.2 发布中,`announce` job 在上传完全部资产后的 undraft 步骤收到 403,Release 停留在 draft,直到在 workflow 之外用带 workflow 修改授权的凭据人工公开。根因不是仓库权限配置错误:GitHub 的 release PATCH 端点规定,当「resolved target commit」的 `.github/workflows/` 文件与默认分支不一致时,令牌必须被授权修改 workflow,而 `GITHUB_TOKEN` 永远无法获得该授权。v1.0.2 的 target commit `1dc76fb`(tag 提交)与 announce 运行时的默认分支 `dd04dd4` 在 `codeql.yml`、`release-plz.yml` 上存在差异——差异来自发布窗口内合入的 #140。

## 症状

- `announce` job 的 `Create GitHub Release` 步骤失败:`HTTP 403: Resource not accessible by integration`,URL 为 `releases/409630075` 的 PATCH 端点(与 `gh release edit` 的调用一致)。
- 同一 bash 步中 `gh release upload "v1.0.2" artifacts/*` 已成功、draft 上 14 个资产齐全,draft 从未公开;被拒绝的只有 undraft。
- run `38135855259` 整体 failure:announce 于 2026-10-11T11:43:00Z 启动、11:43:07Z 失败;`release-extras` 在此之前已全部通过。

## 没有奏效的做法

- 按「权限配置缺失」排查:`release.yml` 顶层已声明 `contents: write`(`.github/workflows/release.yml:17-18`),announce 继承该权限,同一 token 刚成功上传全部资产;增加 `permissions` 解释不了这次 403(session history)。
- 套用「删 draft 后重跑」协议:该协议解决跨 run 的 draft 资产复用(见 [delete-draft-before-rerunning-release-workflow.md](delete-draft-before-rerunning-release-workflow.md)),但本次触发条件是 target 与默认分支的 workflow 文件差异——删 draft 不改变该差异,重跑 announce 会对同一 target 再次 PATCH,必然再收到 403(session history;由已确认根因推断)。
- 改写 Release 的 `target_commitish` 做验证探针:收到 422(字段无效),未产生有效证据,且属于对发布流程对象的无谓写操作(session history)。

## 解决方案

**恢复(本次实际路径)**:v1.0.2 于 2026-10-11T12:11:59Z 公开(`isDraft: false`,可经 API 复核);该次公开在 workflow 之外执行,凭据带 workflow 修改授权、非 `GITHUB_TOKEN`(操作者身份无法经公开 API 复核,此处按会话记录陈述,session history)。随后按「发布验证」清单复核通过(14 个资产、校验和、attestation、双平台镜像 digest 全部一致;Linux 归档校验和、attestation 与镜像 digest 均已独立复核)。命令形态:

```bash
# 在本地(非 CI)环境执行;凭据需带 workflow scope(OAuth / PAT classic)或 Workflows 写权限(fine-grained)。
gh auth status
gh release view v1.0.2 --json isDraft,assets,publishedAt
gh release edit v1.0.2 --draft=false
```

undraft 即公开,执行前先确认 draft 资产齐全;恢复后按 `docs/development.md:76-89` 的「发布验证」复核。

**诊断三步**(下次遇到 announce 403 时):

1. 确认失败点是 release PATCH 端点:`gh run view <run-id> --log-failed` 中 403 的 URL 形如 `releases/<id>`;upload 成功而 undraft 失败是本案特征。
2. 比较 tag 提交与默认分支的 workflow 文件:`git fetch origin main && git diff --stat <tag-commit> origin/main -- .github/workflows/`,有输出即存在差异。
3. 有差异时不要删 draft、不要重跑(必再 403);用带 workflow scope 的本地凭据 undraft。

**防复发**:从 release PR 合并(tag 即将创建)到 announce 完成的发布窗口内,不把改动 `.github/workflows/` 下文件的提交合入默认分支——包括 Dependabot 的 actions 组 PR(#140 即此类)。v1.0.3 的发布窗口内无 workflow 改动、tag 提交即默认分支 tip,announce 全程无人工介入。

## 为什么有效

GitHub REST「Update a release」(`PATCH /repos/{owner}/{repo}/releases/{release_id}`)的官方描述(抓取自官方 OpenAPI 文档,2026-10-12):

> If the resolved target commit (the new value of `target_commitish` if you are changing it, otherwise the existing target) adds or modifies any file under `.github/workflows/` relative to the repository's default branch, the authenticating token must be authorized to modify workflows. Otherwise, this endpoint returns `404 Not Found`; some authentication paths surface `403 Resource not accessible by integration` instead.

> OAuth app tokens and personal access tokens (classic) need the `workflow` scope when the resolved target commit modifies workflow files. … The `GITHUB_TOKEN` available to GitHub Actions cannot be authorized for this; …

规则判定看的是 resolved target commit 与默认分支的 workflow 文件是否一致,差异方向不限:本次实例中差异由默认分支一侧引入(tag 创建后合入了 #140 的 workflow 改动),同样被拒绝(实证)。announce 的实现恰好同时命中两个条件:它 PATCH 时显式传 `--target "$RELEASE_COMMIT"`(`.github/workflows/release.yml:354`,其中 `RELEASE_COMMIT` 为 `github.sha`,即 dispatch 的 tag 提交,`.github/workflows/release.yml:349`),且使用 job 内 `GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}`(`.github/workflows/release.yml:329`)。`GITHUB_TOKEN` 无法被授予 workflow 修改授权,所以只要 target 与默认分支存在 workflow 文件差异,该 PATCH 必被拒绝。

对照:v1.0.1(run `37162008830`)与 v1.0.3(run `38208418042`)发布时,tag 提交与当时默认分支的 workflow 文件一致,announce 均正常公开。v1.0.2 的时间线:tag 创建 11:31:06Z → run 启动 11:31:08Z → #140 合并 11:33:20Z(默认分支出现 workflow 差异)→ announce 11:43:07Z 被拒 → 12:11:59Z 由本地凭据公开。

上游同根因报告:[axodotdev/cargo-dist#2415](https://github.com/axodotdev/cargo-dist/issues/2415)(open,2026-05-31)记录该 token 限制在 cargo-dist 另一处(`gh release create --target`、非分支可达 commit)的同类 403,并列出「tag 已存在时省略 --target」等候选修复方向;上游未落地前,本仓库按上述恢复路径处置。

## 预防

- 发布窗口(从 release PR 合并、tag 创建,到 announce 完成)内不合入改动 `.github/workflows/` 下文件的提交;Dependabot 的 actions 组 PR 也会改 workflow 文件,合入前先确认没有进行中的发布。
- 看到 announce 403 时先跑诊断三步(上文),不要先怀疑 `permissions` 配置——顶层 `contents: write` 已足够完成 upload,不足的是 `GITHUB_TOKEN` 永远拿不到的 workflow 修改授权。
- 不要把本失败模式套用「删 draft 后重跑」协议;两篇文档的适用边界互斥:资产混用问题删 draft,授权问题换凭据。
- 恢复动作只做 undraft,不覆盖已上传资产、不改 target;完成后按「发布验证」复核,再继续后续发布。

## 相关

- [delete-draft-before-rerunning-release-workflow.md](delete-draft-before-rerunning-release-workflow.md) — 相邻恢复协议;适用边界互斥:该文解决跨 run draft 资产复用,本问题解决 `GITHUB_TOKEN` 无法获得的 workflow 修改授权。
- [release-plz-pat-required-to-trigger-pr-workflows.md](release-plz-pat-required-to-trigger-pr-workflows.md) — 同为「`GITHUB_TOKEN` 能力不足、需独立凭据」的平台限制模式;机制不同(事件抑制 vs workflow 修改授权)。
- [docs/development.md](../../development.md) — 发布流程与失败恢复手册;失败恢复段覆盖「删 draft 重跑」(`docs/development.md:72`)与 403 边界及恢复入口(`docs/development.md:74`)。
- [ADR 0012](../../../plans/adr/0012-release-artifacts-and-supply-chain.md) — draft 编排决策;announce 的 upload→undraft 语义来源(#41 修订)。
- 上游 [axodotdev/cargo-dist#2415](https://github.com/axodotdev/cargo-dist/issues/2415) — 同 token 限制的相邻报告(open);其状态是本学习的退役检查项。
- 仓库 issue #44(发布权限面,open)。

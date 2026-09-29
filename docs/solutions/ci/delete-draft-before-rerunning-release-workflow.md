---
title: "Delete the draft before rerunning a cargo-dist release"
date: 2026-09-29
category: ci
module: cargo-dist draft release recovery
problem_type: workflow_issue
component: ci
severity: high
applies_when:
  - "自持 draft 的 cargo-dist 发布流程在 release-extras 或 announce 失败/取消后准备重跑时"
  - "release-extras 对 extras 资产采用“存在则跳过”策略时"
  - "announce 只断言资产名称集合，而不验证校验和或构建来源一致性时"
  - "同一 tag 的重跑会重新构建 dist 产物，但 draft 可能保留上一次运行附加的资产时"
symptoms:
  - "取消或失败的 draft 上已残留 `SHA256SUMS`、SBOM 或镜像 digest 资产"
  - "重跑时已有 extras 被跳过，而归档与 `sha256.sum` 由新一轮构建生成"
  - "draft 可能混用旧 extras 与新 dist 产物"
  - "名称集合断言仍通过，混合内容继续进入公开流程"
  - "`SHA256SUMS` 可能与新的 `sha256.sum` 或归档不一致"
root_cause: missing_workflow_step
resolution_type: documentation_update
related_components: [documentation]
retire_when: "发布流程改为自动清理并重建 draft、对 extras 做内容校验，或 cargo-dist 原生支持“上传但不公开”；检查 dist-workspace.toml 的 github-release 配置与 release-extras 的幂等/校验步骤是否同时改变"
tags: [cargo-dist, draft-release, release-recovery, rerun-idempotency, release-integrity, checksum-consistency, github-actions, fail-closed]
---

# 重跑 cargo-dist 发布前先删除 draft

## Context

Stunt Double 的 cargo-dist 发布采用全程 draft 编排。`dist-workspace.toml` 把 `create-release` 设为 `false`、`github-release` 指向 `announce`，并把 `release-extras` 登记为 publish job（`dist-workspace.toml:27-35`）：GitHub Release 先以 draft 承载资产，只有公告前的验证全部通过，announce 才公开它。

单次 workflow run 内的顺序如下：

1. 该 job 先从当前 run 下载 dist artifacts（`.github/workflows/release-extras.yml:48-53`）。
2. 然后调用 `.github/scripts/ensure-draft-release.sh` 创建或接受 draft。脚本在缺失时用 plan manifest 的标题和正文创建 draft（`.github/scripts/ensure-draft-release.sh:36-51`），已存在 draft 时直接继续，Release 已公开时拒绝操作（`.github/scripts/ensure-draft-release.sh:26-33`；调用点 `.github/workflows/release-extras.yml:55-63`）。
3. 随后生成 CycloneDX SBOM，把当前 run 生成的 checksum 汇总文件 `sha256.sum` 复制成 `SHA256SUMS` 并附加这两个 extras，再解析、验证和记录 GHCR 镜像 digest。三个 extras 资产不是无条件覆盖。`SHA256SUMS` 只在 draft 上不存在同名资产时上传，同名时输出“leaving it unchanged”并跳过（`.github/workflows/release-extras.yml:88-96`）；SBOM 同样按名称跳过（`.github/workflows/release-extras.yml:103-108`）。镜像 digest 文件更严格：已有文件内容与本次 digest 相同才跳过，不同则拒绝覆盖（`.github/workflows/release-extras.yml:242-261`）。
4. 发布前断言把“draft 当前已有资产名”和“announce 即将上传的本地 artifacts 文件名”合并，再交给 `.github/scripts/check-release-assets.sh`（`.github/workflows/release-extras.yml:281-299`）。校验脚本从 dist manifest 的 `.artifacts` 派生 dist 清单，再补上三个 extras 名称（`.github/scripts/check-release-assets.sh:19-33`），但实际比较方式是逐项 `grep -Fxq` 查找资产名（`.github/scripts/check-release-assets.sh:41-48`），不比较资产内容或校验哈希值。
5. announce 只在 `release-extras` 为 `skipped` 或 `success` 时运行（`.github/workflows/release.yml:326`），并在同一个 bash 步骤中先执行 `gh release upload ... artifacts/*`，再执行 `gh release edit ... --draft=false`（`.github/workflows/release.yml:342-354`）。upload 失败会阻止后续 undraft。

这套编排解决了“验证失败前不得公开”的问题，但跨 workflow run 重试还有一个状态问题：draft 和它已附加的 extras 会留在 GitHub Release 上，而每次重新 dispatch `release.yml` 都会重新构建或重新取得该 run 的 dist artifacts。`ensure-draft-release.sh` 会接受已有 draft，因此直接重跑时，`release-extras` 可能在名称相同的旧 `SHA256SUMS`/SBOM 上跳过上传，随后 announce 又上传本次 run 的新 dist 产物。名称集合断言仍会通过，因为旧 extras 和新 dist 产物的名字都在。

（session history）这条恢复约束并非一开始就成立。2026-09-27 的探索曾尝试用 cargo-dist 0.33.0 的 `create-release = false` 直接解 issue #41，结论是该选项假定 draft 已存在，undraft 仍在 dist host 阶段完成；2026-09-29 复查 `github-release = "announce"` 后确认，undraft 仍由 host 阶段的 `dist host --steps=upload --steps=release` 完成，把 extras 移到 publish-jobs 或 post-announce 都无法得到“全部 extras 验证完成后再公开”。结论是发布编排本身需要重构（PR #112），而不是换一个配置开关就能解决。恢复协议最初由这些生命周期语义推导；2026-09-29 的 v0.5.3 取消演练补上了真实失败路径的实测证据（ADR 0012 的 #41 修订）。

## Guidance

失败或取消后的恢复协议是：只要 Release 仍是 draft，就在下一次完整重跑前删除 draft，但保留 tag。让每次恢复都从未附加任何资产的 Release 开始，再完整 dispatch 发布 workflow。

推荐流程：

```bash
tag=v0.5.3

# 1. 确认目标仍未公开。若不存在 draft，也可以直接进入完整 dispatch。
gh release view "$tag" --json isDraft,publishedAt,url

# 2. 删除 draft，保留 tag。不要只清理某一个 extras 文件。
gh release delete "$tag" --yes

# 3. 从 tag 重新触发完整 workflow；必须用 --ref <tag>。
gh workflow run release.yml --ref "$tag" -f "tag=$tag"
```

该顺序已写入 `docs/development.md` 的失败恢复段：上一次运行若已在 draft 上附加 extras（SBOM、`SHA256SUMS`、镜像 digest）或部分上传 dist 产物，必须先删除 draft 再重跑；前者会被幂等检查复用并与新构建的 dist 产物混用，后者会让 upload 因资产重名失败（`docs/development.md:72`）。ADR 0012 的 #41 修订也统一了恢复命令，并指出 `--ref <tag>` 与 release-plz 的 dispatch 路径一致（`plans/adr/0012-release-artifacts-and-supply-chain.md:96`；`.github/workflows/release-plz.yml:44`）。

执行时遵守以下规则：

- 把 draft 视为“某一次 workflow run 的未发布容器”，不要在保留旧 draft 的前提下补跑。
- 删除 draft 不需要删除 tag。删除后，新一轮的 `ensure-draft-release.sh` 会依据新 plan 重新创建 draft，标题和正文也从该 plan 重建。
- 必须重新 dispatch 完整 `release.yml`，不要只重跑某个失败 job，也不要手工把旧 extras 与新 dist 产物拼在一起。
- 不要覆盖、删除或重新发布已经公开的 Release。公开后才发现产物问题，应发布新的 patch 版本；只有 crates.io 发布损坏时才按文档使用 `cargo yank`（`docs/development.md:72`）。
- 删除 draft 只解决 GitHub Release 上的资产混用。GHCR 是另一个状态面：同一 tag 已存在时，`release-extras` 会复用其 digest 并跳过镜像构建（`.github/workflows/release-extras.yml:123-173`）；恢复后应继续让工作流验证镜像覆盖 `linux/amd64` 与 `linux/arm64`，并验证该 digest 的 attestation。镜像 digest 文件若与本次解析到的 digest 不同，会 fail closed，而不是静默混用。
- 完成恢复后，以 `isDraft=false`、公开时间和最终资产清单作为成功判据；名称完整不代表校验内容已做端到端比对。

## Why This Matters

核心风险不是“旧文件占了名字”这么简单，而是校验边界的错位：dist 归档、逐文件校验、`sha256.sum` 等在恢复 run 中重新产生，但已附加的 `SHA256SUMS` 和 SBOM 会按名称视为可用；最终断言只检查必需名称是否存在，不读取旧 `SHA256SUMS` 去核对新归档，也不重新生成 SBOM 后比较内容。因此，**在重跑产生不同字节或不同 manifest 内容的条件下**，保留旧 draft 可能让 Release 公开出一个名称齐全、内容却跨 run 混合的资产集合。这是依据 `.github/workflows/release-extras.yml:88-108`、`.github/workflows/release-extras.yml:281-299` 与 `.github/scripts/check-release-assets.sh:41-48` 得出的防护性推断，不是说 v0.5.3 实际公开过不一致的校验文件。

先删除 draft 可以让 extras 在恢复 run 中重新生成、重新附加，并使发布前断言针对同一轮 run 的 manifest 与本地 artifacts。对于 GHCR 镜像是例外：它的 tag 不会因删除 draft 而消失，现状明确选择复用不可变 tag 的现有 digest，而不是覆盖它；digest 文件与平台/attestation 验证仍把复用限制在一个可验证对象上。

announce 的 upload→undraft 顺序是 fail-closed 的关键。上传和去除 draft 位于同一 bash step（`.github/workflows/release.yml:342-354`），前一步失败时不会执行后一步；同时 `release-extras` 未成功或明确跳过时，announce 也不会启动（`.github/workflows/release.yml:326`）。所以中断留下的应是一个仍未公开的 draft，可以在清理后安全重试。

v0.5.3 首次真实发布中的取消演练验证了这条安全边界，也暴露了恢复协议为什么必须删除 draft：run `36580305336` 在 draft 阶段取消后，检查确认 `isDraft` 仍为 `true`，从未公开；删除 draft 后，run `36581347954` 完整成功，最终 v0.5.3 为 `isDraft=false`。演练当时 draft 上已附加了本轮生成的 `SHA256SUMS` 与 SBOM（镜像 digest 尚未附加；本次演练观察），这正是“保留旧 draft 补跑”不安全的直接条件。ADR 0012 保存了取消、清理与重跑的审计轨迹（`plans/adr/0012-release-artifacts-and-supply-chain.md:97`）；编排和恢复手册分别由 PR #112、#113 合入；`docs/development.md` 记录正常流程、失败恢复与取消演练要求。

## When to Apply

在以下场景执行“先确认未公开，再删除 draft，再完整 dispatch”：

- `release.yml` 在 `release-extras` 阶段失败或取消，draft 上可能已有 extras 或部分 dist 资产。
- announce 阶段失败或取消，draft 上可能已部分上传 dist 产物。
- 为同一 tag 再次手工触发 `release.yml`，且该 tag 已存在非公开 draft。
- 对发布编排做结构性修改后执行失败路径演练；升级 cargo-dist 后还应按 `docs/development.md` 的「失败路径演练」重做结构检查，确认 announce 的 Cleanup、upload 顺序和 `find` 集合仍与断言一致。
- 发现 Release 已公开时不要套用本协议。此时应保留已公开事实，修问题并发新的 patch 版本。

该协议依赖当前实现：`ensure-draft-release.sh` 接受已有 draft，extras 按名称复用 `SHA256SUMS`/SBOM，名称断言不比较内容。如果以后改为在 workflow 内自动删除并重建 draft、对 extras 做内容校验，或由 cargo-dist 提供等价的“上传但不公开”原语，应在新实现和演练通过后更新或退役本学习。

## Examples

**正确恢复：删除 draft 后完整重跑。** 下面是 v0.5.3 的同类路径：

```bash
tag=v0.5.3
gh release view "$tag" --json isDraft,publishedAt,url
gh release delete "$tag" --yes
gh workflow run release.yml --ref "$tag" -f "tag=$tag"
```

run `36580305336` 的取消演练确认 draft 未公开；run `36581347954` 是清理 draft 后完整发布成功的证据。恢复后核对：

```bash
tag=v0.5.3
gh release view "$tag" --json isDraft,publishedAt,assets
gh run view 36581347954 --json status,conclusion,jobs
```

期望状态是 workflow run 成功、`isDraft` 为 `false`，且资产同时包含 dist manifest 声明的产物、`SHA256SUMS`、`stuntdouble-<version>.cdx.json` 和 `stuntdouble-<version>-image.txt`。

**错误恢复：保留旧 draft 直接 dispatch。** 不要执行：

```bash
# 错误：已有 draft 与旧 extras 时直接重跑。
gh workflow run release.yml --ref v0.5.3 -f "tag=v0.5.3"
```

在这条路径上，`ensure-draft-release.sh` 会接受已有 draft（`.github/scripts/ensure-draft-release.sh:26-33`）；`SHA256SUMS` 和 SBOM 会因同名而被跳过（`.github/workflows/release-extras.yml:88-108`）；最终校验仍可能仅因名称齐全而通过（`.github/scripts/check-release-assets.sh:41-48`），随后新 dist 产物被上传并公开。若恢复 run 的构建内容与旧 extras 不一致，这正是跨 run 混用的条件；在构建一致时它可能不表现为立即失败，但不能作为恢复协议的依据。

## Related

- [derive-required-release-assets-from-dist-manifest.md](derive-required-release-assets-from-dist-manifest.md) — 同样处理 `release-extras` 的公告前资产断言，但那篇解决“必需资产清单硬编码漂移”，本篇解决“名称齐全的资产仍可能跨 run 混合”。names-only 断言正是本篇风险能穿过的边界。
- [docs/development.md](../../development.md) — 发布流程、失败恢复命令与失败路径演练的权威手册。
- [ADR 0012](../../../plans/adr/0012-release-artifacts-and-supply-chain.md) — draft 编排的决策记录与 v0.5.3 取消演练审计轨迹（#41 修订）。
- [cross-compile-arm64-container-image-on-amd64-runner.md](cross-compile-arm64-container-image-on-amd64-runner.md) — 相邻的 `release-extras` 公告前验证面；那篇管镜像平台覆盖，不解决 draft 状态复用。
- issue #41（发布原子性）、#43（从 dist manifest 派生资产清单）、#112（draft 编排实现）、#113（恢复手册与 draft 生命周期防护）、#114（v0.5.3 发布列车）。

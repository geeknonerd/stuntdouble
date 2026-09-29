---
title: "Derive required release assets from the cargo-dist manifest"
date: 2026-09-29
category: ci
module: release-extras asset verification
problem_type: workflow_issue
component: ci
severity: medium
applies_when:
  - "Adding, removing, or renaming release assets, including supported apiVersion type definitions"
  - "Changing the cargo-dist plan input or the release-extras required-asset assertion"
  - "Deciding whether a release checklist should be hardcoded in multiple surfaces or derived from the plan manifest"
  - "Reviewing CI coverage for release validation logic"
symptoms:
  - "The required release asset list is hardcoded in release-extras.yml and repeated in docs/development.md"
  - "A new apiVersion type definition or asset class requires manual edits in several places"
  - "A missed list update is exposed only when release verification runs late"
  - "The previous assertion missed per-file .sha256 files and source archives that cargo-dist already produced"
root_cause: missing_workflow_step
resolution_type: workflow_improvement
related_components: [documentation]
tags: [release-assets, cargo-dist, dist-manifest, single-source-of-truth, release-automation, github-actions, self-test, fail-closed]
---

# Derive required release assets from the cargo-dist manifest

## 背景（Context）

`.github/workflows/release-extras.yml` 曾把发布必需资产写成工作流内的 `required=(...)` 数组，`docs/development.md` 又维护一份同义清单。新增受支持的 `apiVersion` 类型定义或新的资产类别时，至少要同步 dist 配置、发布断言和发布文档；漏改断言或文档不会被旧断言发现——它只对照自己的硬编码清单，新资产可能静默漏检；只有清单已列出而发布缺产时，才会在发布验证后期报缺资产。issue #43 记录了该缺口，PR #108 将其改为从 dist manifest 派生。

会话实测：plan job 的真实输入来自 `dist host --steps=create --tag=... --output-format=json`（`.github/workflows/release.yml:82-87`），该输出与 `dist plan` 的 `artifacts` 一致；因此 `.artifacts` 是 manifest 显式声明为 artifacts 的 dist 产物集合的安全来源；workflow 另行上传的发布元数据（如 `dist-manifest.json`）不在该集合内。

当前实现由 `.github/scripts/check-release-assets.sh` 读取 plan：`required_assets()` 从 `.artifacts` 的键派生 dist 产物，再补 release-extras 自产的资产（`.github/scripts/check-release-assets.sh:19-33`）。`release-extras` 把 plan 与 GitHub Release 的实际资产名交给脚本（`.github/workflows/release-extras.yml:267-280`）；开发文档只指向该脚本作为权威来源（`docs/development.md:88`），ADR 0012 也有 #43 修订（`plans/adr/0012-release-artifacts-and-supply-chain.md:79-85`）。

（session history）#43 曾一度被归入「等待 cargo-dist 上游能力」而搁置；2026-09-29 重新调研发现，plan job 已经能把完整 dist manifest 交给 `release-extras`，用仓库现有版本即可派生，无需上游新能力，于是改为立即可推进的候选。

## 做法（Guidance）

1. **把 plan 的 `.artifacts` 键作为 dist 产物的权威清单，不再维护第二份清单。** plan job 输出 manifest（`.github/workflows/release.yml:82-87`），经 `needs.plan.outputs.val` 传给 release-extras（`.github/workflows/release.yml:343-345`）；后者把 `inputs.plan` 写入临时文件并调用脚本（`.github/workflows/release-extras.yml:269-280`）。脚本只需在 `required_assets()` 中派生一次：

   ```bash
   jq -r '.artifacts | keys[]' "$plan_file"
   ```

   这一行位于 `.github/scripts/check-release-assets.sh:27`。不要在工作流或文档中再复制 `ctx-api-v1.d.ts`、三个平台归档、逐文件 `.sha256`、源码归档、`sha256.sum` 等名单。

2. **只静态声明 release-extras 自己生成的 3 项。** 脚本在 `.github/scripts/check-release-assets.sh:29-32` 声明 `SHA256SUMS`、`stuntdouble-${version}.cdx.json`、`stuntdouble-${version}-image.txt`。它们分别由 checksum 步骤（`.github/workflows/release-extras.yml:64-77`）、SBOM 步骤（`.github/workflows/release-extras.yml:44-62`、`:79-89`）和镜像引用步骤（`.github/workflows/release-extras.yml:223-242`）产生或附加。会话实测：这些 post-announce 资产不出现在 dist manifest 中，所以脚本必须补这一小段；但只应补“dist 之外、本 job 自产”的部分。

3. **`.artifacts` 为空必须 fail closed，不能让空集合通过。** `required_assets()` 在 `.artifacts` 长度为 0 时输出 `the dist manifest declares no artifacts to verify` 并返回 1（`.github/scripts/check-release-assets.sh:21-24`）；`check()` 传播该失败（`.github/scripts/check-release-assets.sh:38-40`）。这覆盖 manifest 生成或解析异常导致空对象的情况，不要用 `|| true` 绕过。

4. **把断言作为对外公告前的发布闸门，并为已公开 Release 配 Withdraw。** release-extras 读取实际资产名后调用脚本（`.github/workflows/release-extras.yml:267-280`）；脚本逐项用 `grep -Fxq` 检查并汇总全部 `missing`（`.github/scripts/check-release-assets.sh:41-48`）。该 job 是 post-announce job（`dist-workspace.toml:29-30`），所以失败或取消时由 `Withdraw an incomplete release` 把 Release 改回 draft（`.github/workflows/release-extras.yml:282-291`）；开发文档也规定全部资产存在后才公告（`docs/development.md:68`）。断言失败不能让流程带着不完整 Release 正常结束。

5. **用 `--self-test` 覆盖“新增声明应通过”和“缺声明必须失败”，并放进必需的 `test` job。** self-test 构造包含虚构 `ctx-api-v2.d.ts` 的 plan（`.github/scripts/check-release-assets.sh:57-65`），完整 actual 应通过（`.github/scripts/check-release-assets.sh:67-80`）；随后从 actual 删除该文件，检查必须失败（`.github/scripts/check-release-assets.sh:82-95`），全部通过时输出 `self-test passed`（`.github/scripts/check-release-assets.sh:98`）。CI 的 `test` job 运行它（`.github/workflows/ci.yml:54-55`）。`ctx-api-v2.d.ts` 只是测试夹具，不代表仓库已经支持 apiVersion 2。

6. **文档只描述来源与例外，不复制派生清单。** `docs/development.md:88` 指定脚本为权威来源，并单独列出 release-extras 自产资产；ADR 0012 的 #43 修订记录同一决策（`plans/adr/0012-release-artifacts-and-supply-chain.md:79-85`）。新增类型定义时应先进入 `dist.extra-artifacts`（`dist-workspace.toml:40-42`），由 manifest 派生覆盖，而不是再次编辑发布工作流内的数组。

## 为什么重要（Why This Matters）

- **单一来源消除多点漂移。** 旧实现同时维护 workflow 数组和文档清单；新的 dist 产物只要出现在 plan 的 `.artifacts` 中，断言自动覆盖，新增类型定义或资产类别不再要求在工作流与文档重复登记。
- **派生断言比旧版更严格，属于行为变化。** 会话实测：`.artifacts` 让断言额外覆盖逐文件 `.sha256`、`source.tar.gz` 及其 `.sha256` 等旧清单未逐项列出的条目（`sha256.sum` 旧清单已有，但新实现不再手写它），因此发布必须具备的集合比旧的 8 项列表更完整。
- **空集合 fail closed 防止“零资产也通过”。** 如果没有显式判空，`.artifacts | keys[]` 没有任何输出，循环可能不执行并返回成功；当前实现用明确错误阻断这条路径（`.github/scripts/check-release-assets.sh:21-24`）。
- **self-test 让发布逻辑进入常规 CI。** 正例证明新声明会自动成为必需资产，负例证明漏传会失败；否则脚本改动只能等到真实发布才暴露。
- **Withdraw 让发布闸门有实际后果。** 对已经公开的 Release，失败或取消会将其退回 draft（`.github/workflows/release-extras.yml:282-291`），避免留下不完整但仍对外可见的发布。
- **静态三项没有被“派生”错误吞掉。** manifest 只描述 dist 产物；release-extras 自产的 checksum、SBOM 和镜像引用不在其中，因此仍需在脚本中显式声明并在 self-test 中覆盖。

## 何时适用（When to Apply）

- 发布工作流已有由构建工具生成、且能作为权威来源的 manifest 或 plan 时。
- 一个 post-announce/custom job 在 manifest 之外额外生成或附加发布资产时。
- 新增受支持 `apiVersion` 类型定义、新增资产类别、或改变 dist 目标与额外产物时。
- 断言输入可能为空，且空输入会导致“没有检查任何东西却成功”时。
- 发布完成前存在人工公告步骤，或平台允许失败后把 Release 退回 draft 时。
- 没有权威 manifest 时，不要伪造“派生”；应先建立单一来源，或保留显式清单并增加清单漂移测试。

## 示例（Examples）

**改动前：工作流内的硬编码数组。** PR #108 之前，`release-extras` 直接列举全部必需资产（历史片段取自 `git show 231bc31^:.github/workflows/release-extras.yml`）：

```bash
required=(
  "ctx-api-v1.d.ts"
  "SHA256SUMS"
  "sha256.sum"
  "stuntdouble-${version}.cdx.json"
  "stuntdouble-${version}-image.txt"
  "stuntdouble-aarch64-apple-darwin.tar.gz"
  "stuntdouble-x86_64-pc-windows-msvc.zip"
  "stuntdouble-x86_64-unknown-linux-gnu.tar.gz"
)
```

新增 `ctx-api-v2.d.ts`、源码归档或逐文件校验和后，必须同时修改该数组；漏改断言时新资产不会被检查（静默漏检），只有数组已列出而发布缺产时才会在验证阶段报缺资产。

**改动后：把 plan 与实际资产交给脚本。** workflow 只负责取得 manifest 和 Release 的实际资产名，不再保存清单（等价于 `.github/workflows/release-extras.yml:275-280`）：

```bash
plan_file="$(mktemp)"
actual_file="$(mktemp)"
printf '%s' "$PLAN" > "$plan_file"
gh release view "$TAG" --repo "$GITHUB_REPOSITORY" --json assets \
  --jq '.assets[].name' > "$actual_file"
./.github/scripts/check-release-assets.sh "$plan_file" "$actual_file" "$version"
```

脚本内部先执行 `jq -r '.artifacts | keys[]' "$plan_file"`，再补 `SHA256SUMS`、`stuntdouble-${version}.cdx.json`、`stuntdouble-${version}-image.txt`。

**self-test 的正例夹具。** plan 虚构新增 `ctx-api-v2.d.ts`；actual 同时包含 dist 产物与 release-extras 自产三项时，检查应通过（摘自 `.github/scripts/check-release-assets.sh:57-74`）：

```json
{
  "artifacts": {
    "ctx-api-v1.d.ts": {},
    "ctx-api-v2.d.ts": {},
    "stuntdouble-x86_64-unknown-linux-gnu.tar.gz": {}
  }
}
```

```text
ctx-api-v1.d.ts
ctx-api-v2.d.ts
stuntdouble-x86_64-unknown-linux-gnu.tar.gz
SHA256SUMS
stuntdouble-0.5.2.cdx.json
stuntdouble-0.5.2-image.txt
```

**self-test 的负例夹具。** 从 actual 删除 `ctx-api-v2.d.ts` 后，检查必须返回非零并输出缺项（摘自 `.github/scripts/check-release-assets.sh:82-95`）：

```text
ctx-api-v1.d.ts
stuntdouble-x86_64-unknown-linux-gnu.tar.gz
SHA256SUMS
stuntdouble-0.5.2.cdx.json
stuntdouble-0.5.2-image.txt
```

```text
missing release asset: ctx-api-v2.d.ts
```

运行完整回归：

```bash
./.github/scripts/check-release-assets.sh --self-test
# 实际输出：
# missing release asset: ctx-api-v2.d.ts
# self-test passed
```

会话实测：PR #108 的 CI 也曾先输出该缺项，再输出 `self-test passed`。

## 相关（Related）

- [cross-compile-arm64-container-image-on-amd64-runner.md](cross-compile-arm64-container-image-on-amd64-runner.md) — 同属 `release-extras` 公告前的发布完整性断言；那篇管平台覆盖，本篇管 manifest 派生与 extras 的必需资产断言。
- [ghcr-package-visibility-follows-the-publishing-token.md](ghcr-package-visibility-follows-the-publishing-token.md) — 同为 `release-extras` 验证面，但那篇记录了为什么可见性不为 GitHub 默认行为新增门禁。
- [../conventions/slice-status-surface-sweep.md](../conventions/slice-status-surface-sweep.md) — 同样处理多表面信息漂移；本篇的解法是把权威来源收窄为 manifest 派生。
- [../conventions/refresh-documentation-code-anchors-by-semantics-not-offset.md](../conventions/refresh-documentation-code-anchors-by-semantics-not-offset.md) — 复核本篇引用的 `file:line` 锚点时使用语义重定位，而不是偏移量换算。
- issue #43（本篇）、#67（同工作流的平台覆盖断言）、#41（发布原子性，独立跟踪）。

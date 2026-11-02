---
title: "macOS 14 runner brownout cancels the cargo-dist release build"
date: 2026-11-02
category: ci
module: cargo-dist release pipeline
problem_type: build_error
component: ci
root_cause: tool_limitation
resolution_type: config_change
severity: high
retire_when: "macos-14 重新可用（已退役，不会发生），或 cargo-dist 默认不再把 aarch64-apple-darwin 映射到已退役的 macOS 镜像；检查方式：本地跑 `dist plan` 并确认 artifacts_matrix 中 macOS 构建的 runner 不再是已退役镜像"
tags: [cargo-dist, release, macos, runner, brownout, github-actions, dist-workspace, github-custom-runners]
---

# macOS 14 runner brownout 取消 cargo-dist 发布构建

## 问题

v1.8.0 的 `Release` 工作流（Run 41360777445）被取消，`aarch64-apple-darwin` 构建任务在启动 1 秒内被 cancel，随后的 `host`、`build-global-artifacts`、`custom-release-extras`、`announce` 全部级联 skip，导致 tag `v1.8.0` 已打但 GitHub Release 未发布。

Linux x86_64 和 Windows x86_64 构建成功，只有 macOS arm64 受影响，标签已可达 `origin/main`，发布构建前置断言也通过，故问题不在构建内容或信任链，而在 runner 镜像本身。

## 症状

- `build-local-artifacts (aarch64-apple-darwin)` 的 `runs_on` 为空，`steps` 为空，结论 `cancelled`。
- 失败原因是 macOS 14 runner 镜像 brownout：GitHub Actions 在 2026-11-02 全天预演 macOS 14 Sonoma 镜像退役（actions/runner-images#13518），分配到该镜像的任务被立即取消。
- cargo-dist 0.33 默认把 `aarch64-apple-darwin` 映射到 `macos-14`，故触发失败。

## 没有奏效的做法

- 不应手工编辑 `.github/workflows/release.yml`：它由 `dist-workspace.toml` 通过 `dist generate` 生成，`release-drift` 会拒绝与 `dist plan` 的 `artifacts_matrix` 不一致的手写改动。
- 不应把 `runs-on` 改为具体的新版本镜像（如 `macos-15`）：那样会锁死到某一版本，后续退役时重现同类问题。

## 解决方案

在 `dist-workspace.toml` 中启用 `github-custom-runners`，把 `aarch64-apple-darwin` 固定到移动标签 `macos-latest`，让构建始终跟随 GitHub 当前受支持的 macOS 镜像：

```toml
# macOS 14 runner images retire on 2026-11-02 (actions/runner-images#13518);
# pin the moving macos-latest label so the build follows whatever image is current.
github-custom-runners = { aarch64-apple-darwin = "macos-latest" }
```

然后重新生成并校验：

```bash
dist generate --check          # 校验生成的 release.yml 与配置一致
bash .github/scripts/check-cargo-dist-bootstrap.sh  # 校验 bootstrap 信任链
```

本地验证：

```bash
dist plan --output-format=json | jq -r '.ci.github.artifacts_matrix.include[].runner'
# macos-latest   # 原为 macos-14
```

`dist plan` 生成的 `artifacts_matrix` 会流入 `release.yml` 的 `matrix.runner`，`release-drift` 只校验 bootstrap wrapper 路由与 action pin，不校验 runner，故只改配置文件即可，无需手改生成的 workflow。

失败时 tag 已经由 release-plz 创建，但 tag 指向的 commit 里的 `dist-workspace.toml` 还是旧配置。Release 工作流的 plan job 用 `actions/checkout` 检出触发 ref（即 tag），所以对旧 tag 直接重跑、或删掉失败 run 后重新 dispatch，都仍会算出 `macos-14` 而再次失败。修复必须先合并进 main，再删除并重建同名 tag（tagger 消息沿用 release-plz 生成的 `chore: Release package stuntdouble version <version>`，tag 指向 main 新 HEAD），然后 `gh workflow run release.yml --ref <tag> -f tag=<tag>`。若失败 run 已留下 draft，先按「失败路径演练」删掉 draft 再重跑；本例 run 在 brownout 阶段即被取消，未产生 draft，无需删除。

## 为什么有效

`macos-latest` 始终指向 GitHub 当前受支持的 macOS runner 镜像，不会因单一镜像版本的 brownout 或退役而失效。配置变更后 `dist generate` 产出的 `release.yml` 会把 macOS 构建调度到 `macos-latest`，后续镜像退役只需继续使用 `macos-latest` 即可，不再触发 runner 不可用导致的任务取消。

## 预防

- 将 `dist generate --check` 与 `check-cargo-dist-bootstrap.sh` 纳入每次改 `dist-workspace.toml` 的提交前检查。
- 关注 `actions/runner-images` 的 runner 镜像退役公告（如 macOS 14 在 2026-11-02 退役），提前把受影响平台的 runner 迁到移动标签。
- 不要在 release.yml 中硬编码具体镜像版本；所有 runner 选择都应经 `dist plan` 的 `artifacts_matrix` 产生，配置在 `dist-workspace.toml` 中。

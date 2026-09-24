---
title: "Node.js 20 deprecation annotations only cover actions that target node20"
date: 2026-09-24
category: ci
module: CI action runtimes
problem_type: integration_issue
component: ci
symptoms:
  - "CI 的 `audit` job 每次运行都带 `Node.js 20 is deprecated` 的 warning 注解，指向 `rustsec/audit-check@69366f3`"
  - "`pages build and deployment` 的 `build` job 带同一条注解，指向 `actions/upload-artifact@v4`"
root_cause: deprecated_action_runtime
resolution_type: dependency_update
severity: medium
tags: [github-actions, node-runtime, deprecation, cargo-audit, rustsec, github-pages, required-checks, supply-chain]
---

# Node.js 20 弃用注解只覆盖以 node20 为运行时的 action

## 问题

2026-09-21 起，仓库的 CI 运行开始出现 GitHub 的 Node.js 20 弃用注解（[changelog](https://github.blog/changelog/2025-09-19-deprecation-of-node-20-on-github-actions-runners/)）。注解文本形如「The following actions target Node.js 20 but are being forced to run on Node.js 24: `rustsec/audit-check@69366f3`」。触发它的有两处：仓库可控的 `audit` job，以及 GitHub 托管的 Pages 构建。

## 根因

注解的选择条件是 action 声明的运行时，而不是 job 用的是哪个 runner：

- 只有 `runs.using: node20` 的 action 会被点名。旧的 `node16` / `node12` action（本仓库的 `tim-actions/get-pr-commits`、`tim-actions/dco`）同样早已被强制升级，却不出现在注解里，`dco` 与 `pr-title` 运行只有 `ubuntu-latest` 迁移提示。因此「扫一遍使用了哪些 action」不足以定位问题，必须逐个看 `runs.using`。
- `rustsec/audit-check` 停在 `v2.0.0`（2024-09-23），上游没有更新运行时的版本，本地无法升级，只能换实现。
- `pages build and deployment` 是 GitHub 托管的 legacy Pages 构建（`build_type: legacy`，source 为 `main` 分支的 `/docs`），工作流不在本仓库里，`actions/upload-artifact@v4` 的 pin 由 GitHub 自己决定。

## 处置

`.github/workflows/ci.yml` 的 `audit` job 改为不依赖 Node.js action：

```yaml
      - if: steps.cargo.outputs.exists == 'true'
        uses: dtolnay/rust-toolchain@6bed0761d98439e5a578e2877258200ad565ba87 # stable
      - if: steps.cargo.outputs.exists == 'true'
        run: cargo install cargo-audit --version 0.22.2 --locked
      - if: steps.cargo.outputs.exists == 'true'
        run: cargo audit
```

替换前后的语义对照：

- 仍有漏洞即失败：`cargo audit` 在发现公告时以退出码 1 结束（本地用一个指向 `time 0.1.44` / RUSTSEC-2020-0071 的构造 `Cargo.lock` 验证过退出码，仓库自身 `Cargo.lock` 退出码 0）。
- 未造成能力回退：`audit-check` 的「自动创建/更新 issue」在本仓库从未生效，`ci.yml` 的顶层 `permissions: contents: read` 没有给它 `issues: write`。
- 新增的代价是安装步骤要现场编译：`cargo install cargo-audit --version 0.22.2 --locked` 在 4 核机器上约 4 分半（`--locked` 能用，因为发布的 crate 带 `Cargo.lock`）。tradeoff: 现状是零额外 action 依赖、只走 crates.io；天花板是每次 CI 多花几分钟；当 CI 时长成为瓶颈时，改用预编译二进制或缓存安装结果（RustSec 为 `cargo-audit` 发布了 cargo-dist 产物）。
- `cargo-audit` 版本由 Dependabot 之外的人工维护：GitHub Actions 生态的 Dependabot 看不到 `run:` 里的版本号，升级时要同时改 workflow 与本文件所在节的记录。

## 核查方法

```bash
# 某个 commit 上每个 check run 的注解数量
gh api "repos/geeknonerd/stuntdouble/commits/<sha>/check-runs" \
  --jq '.check_runs[] | [.id, .name, (.output.annotations_count|tostring)] | @tsv'

# 读取注解正文（warning 出现在用了 node20 action 的那个 job 上）
gh api "repos/geeknonerd/stuntdouble/check-runs/<check-run-id>/annotations" \
  --jq '.[] | [.annotation_level, .message] | @tsv'

# 判断某个 SHA 固定的 action 用什么运行时
gh api "repos/<owner>/<repo>/contents/action.yml?ref=<sha>" --jq '.content' | base64 -d | grep -n 'using:'
```

本仓库剩余 pin 的运行时（2026-09-24 逐个核对 `runs.using`）：`node24` 有 `actions/checkout`、`actions/upload-artifact`、`actions/download-artifact`、`actions/attest`、`amannn/action-semantic-pull-request` 与三个 `docker/*` action；`composite` 有 `dtolnay/rust-toolchain`、`lycheeverse/lychee-action`、`actions/attest-build-provenance`、`github/codeql-action`、`release-plz/action`；`EmbarkStudios/cargo-deny-action` 是 `docker`。新增 action 时优先这三类。旧运行时只剩 `tim-actions/get-pr-commits`（`node16`）与 `tim-actions/dco`（`node12`），上游没有更新版本，当前不打注解，暂不替换。

## 仍是外部项

`pages build and deployment` 的 `build` job 注解无法在本仓库修：它由 GitHub 的 legacy Pages 构建生成，只能等 GitHub 更新自管 action。判断依据是 `gh api repos/<owner>/<repo>/pages` 返回 `build_type: legacy`，且 `.github/workflows/` 下没有对应工作流文件。注解消失前不要把它当作仓库回归。

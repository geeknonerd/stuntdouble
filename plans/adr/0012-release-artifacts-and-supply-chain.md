# 可验证的发布产物与供应链基线

- 状态：`已接受`
- 日期：2026-09-19
- 关联：[ADR 0008](0008-dual-mit-apache-license.md)、[ADR 0010](0010-git-and-release-workflow.md)、[开发指南](../../docs/development.md)

## 背景

Stunt Double 面向 CI 与本地开发，使用者需要可信的二进制与容器。无法追溯到 tag、或下载后无法验证的发布，会带来本可避免的供应链风险。

## 决策

每个 release 提供可验证的产物集合：

- GitHub 生成的源码归档
- Linux x86_64、macOS arm64、Windows x86_64 二进制
- 每个受支持 `apiVersion` 的 `ctx` API `.d.ts` 类型定义
- `SHA256SUMS`
- GitHub artifact attestation
- CycloneDX 或 SPDX 格式的 SBOM
- 容器镜像 `ghcr.io/geeknonerd/stuntdouble:<tag>`，并公布 digest
- 由 CHANGELOG 生成的 GitHub Release notes

只从 tag 构建产物，绝不发布从 `main` 构建的二进制。

签名从 GitHub artifact attestation 开始；只有出现离线验证需求时才引入 Sigstore/cosign。

容器镜像使用多阶段构建与最小运行时镜像。tag 与 digest 都要发布；`latest` 绝不作为唯一引用。

发布失败不覆盖已有产物，修复问题后发布新版本。只有 crates.io 发布损坏时才用 `cargo yank`。

## 工具

- `release-plz`：release PR、版本号、CHANGELOG、tag 与可选的 crates.io 发布。
- `cargo-dist`：多平台二进制、安装器、校验和与 GitHub Release 资产。
- GitHub Actions：CI 与发布流程。
- `cargo-deny` 与 `cargo-audit`：依赖与安全公告检查。
- Dependabot：Cargo、Docker 基础镜像与 GitHub Actions 更新。

## 启用时机

发布流程与 `cargo-dist` 配置在第一个实现切片中加入，早于首次公开发布。在此之前，本 ADR 定义的是必须具备的形态，而不是已启用的流水线。

## 后果

- 发布自动化需要访问仓库与包注册表的 secret。
- 产物可以从 tag 与锁定的依赖图复现。
- release notes 是发布产物的一部分，不是事后补充。

## T8/T9 修订（2026-09-22）

T8 与 T9 共同完成本 ADR 的发布目标，但职责分开：

- T8 冻结并发布 `apiVersion` 1 的 `types/ctx-api-v1.d.ts` 源码定义，不激活发布流水线。
- T9（#12）激活 release-plz、cargo-dist、容器、attestation 与 GitHub Release，并把每个受支持 `apiVersion` 的 `.d.ts` 作为必带资产。
- T9 的验收必须包含 GitHub Release 附件验证；在验证完成前，文档只能声明“发布契约要求包含”，不得声明“已经随发布产物提供”。

## T9 实施修订（2026-09-22）

T9 的发布工作流已落地，执行时补充以下约束：

- 发布构建只接受可从 `origin/main` 到达的严格 `vMAJOR.MINOR.PATCH[-prerelease]` tag；tag 与 `Cargo.toml` 版本的一致性由 cargo-dist 校验。
- GHCR tag 不可覆盖。tag 已存在时复用 digest 并验证已有 attestation，不重新生成 provenance；无法确认 tag 是否存在时 fail closed。tag 不存在时才构建、推送并生成 attestation。
- `release-extras` 失败或取消时，把已公开但不完整的 GitHub Release 回退为 draft。完整的 draft 编排仍待解决，跟踪于 #41；2026-09-29 完成 cargo-dist 0.33 能力核实（`create-release=false` 与 `github-release="announce"` 的阶段语义），候选路线为等上游 undraft 能力、或把 `release-extras` 前移为 publish job，尚未决策，核实记录见 #41。
- CodeQL 作为并行安全扫描运行，不加入分支保护的 required checks；是否启用合并保护跟踪于 #45。
- release-plz 的 tag、分支与 release PR 使用 `RELEASE_PLZ_TOKEN`（细粒度 PAT：Contents 与 Pull requests 读写）；触发 cargo-dist 的 `workflow_dispatch` 使用 job `GITHUB_TOKEN` 的 `actions: write`，PAT 不需要 Actions 权限。
- 其余后续硬化项：匿名 GHCR 拉取验证（#42）、资产清单单一来源（#43）、cargo-dist 权限与 installer 摘要（#44）。

## #67 修订（2026-09-24）：容器镜像覆盖两个平台

「决策」中的容器镜像条目此前只由 amd64 runner 构建，发出去的是单平台镜像：`v0.2.1` 的 tag 解析为 amd64 的 image manifest，arm64 主机只能靠模拟运行，而 cargo-dist 早已发布 `aarch64-apple-darwin` 归档。该条目据此收敛为：

- 同一 tag 下发布覆盖 `linux/amd64` 与 `linux/arm64` 的镜像索引。
- amd64 在宿主平台上原生编译；arm64 由同一个宿主平台 builder 交叉编译（`gcc-aarch64-linux-gnu` + `libc6-dev-arm64-cross`），runtime 阶段仍在 QEMU 下安装 `ca-certificates`。不采用全量模拟：本机实测同一 Dockerfile 的模拟构建超过 25 分钟仍未完成，交叉编译的编译步骤 263 秒，与原生 amd64 的 258 秒持平。
- `release-extras` 在公告前断言该 tag 是同时覆盖两个平台的镜像索引，否则 fail closed；`v0.2.1` 及更早的 tag 保持单平台。

原因：镜像与三平台归档并列发布，而用户侧的 arm64（Apple Silicon 上的 Docker、ARM 云主机）很常见；把额外成本放在交叉编译而不是模拟，能保住发布 job 的时长。

## #43 修订（2026-09-29）：必需资产清单单一来源

「T9 实施修订」曾把资产清单单一来源列为后续硬化项。现落地为：

- `release-extras` 的必需资产断言不再硬编码文件名：`.github/scripts/check-release-assets.sh` 从 dist manifest（plan job 的 `artifacts` 字段）派生全部 dist 产物，仅静态声明 release-extras 自产资产（`SHA256SUMS`、`stuntdouble-<version>.cdx.json`、`stuntdouble-<version>-image.txt`）。
- 新增受支持 `apiVersion` 的类型定义或新资产类别无需再编辑工作流内的清单；dist manifest 未声明任何产物时 fail closed。
- `ci.yml` 的 `test` job 运行脚本 `--self-test`：新声明产物随发布附加时通过、缺失时失败。

## #41 修订（2026-09-29）：发布改为 draft 编排

「T9 实施修订」中“失败后回退为 draft”的缓解被替换为全程 draft 编排：GitHub Release 只在全部资产附加并通过断言后公开。

- `dist-workspace.toml` 使用 cargo-dist 0.33 原生配置：`create-release = false`、`github-release = "announce"`，并把 `release-extras` 从 post-announce job 前移为 `publish-jobs` 成员。
- `release-extras` 创建 draft Release（标题/正文取自 plan manifest 的 `announcement_title` / `announcement_github_body`），从 workflow artifacts 读取 dist 产物，附加 SBOM / `SHA256SUMS` / 镜像 digest，并断言“draft 已附加的 extras 与 announce 即将上传的 dist 产物”覆盖 manifest 声明的全部必需资产；原“Withdraw an incomplete release”步骤删除。
- 生成的 announce job 只在 `release-extras` 为 `skipped`/`success` 时运行，并在同一个 bash 步中先上传 dist 产物、再去除 draft；upload 失败即中止，任何失败或取消都不会公开 Release。
- 该编排不依赖上游 axodotdev/cargo-dist#2521 的 undraft 能力。若上游随后提供“上传但不公开”，可再评估是否简化。
- 校验：cargo-dist 0.33.0 `dist generate` 生成结构已实测；`.github/scripts/check-release-assets.sh` 的 `--self-test` 与“完整/缺失”两种模拟断言通过；取消演练安排在下一次真实发布时执行（见 `docs/development.md`「失败路径演练」）。

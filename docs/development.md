# 开发与发布流程

本文件是贡献者与维护者的操作指南，记录 ADR 0010–0013 的决策。

## Git 工作流

- `main` 是唯一长期分支，始终可发布。
- 所有改动走短生命周期分支 + pull request。
- 分支名前缀：`feat/`、`fix/`、`docs/`、`chore/`、`ci/`、`release/`。
- 只允许 squash merge，合并后删除分支。
- 禁止直接 push、force push 与删除 `main`。
- 发布从 `main` 打 tag；不使用长期 `develop` 分支。
- 1.0 之后如需维护旧版本线，从对应 tag 创建 `release/x.y` 分支。

## Pull request

每个 pull request 必须：

- 使用 Conventional Commits 标题，因为 squash commit 会继承它
- 通过 `fmt`、`clippy`、`test`、`docs`、`docs-links`、`deny`、`audit`、`msrv`、`pr-title`、`dco`
- 解决全部 review 会话
- 与 `main` 保持同步
- 每个 commit 带 DCO 签名（`git commit -s`）；release-plz 生成的 release PR 是唯一例外（见「发布流程」）

CodeQL 不是 required status check，而是由仓库 ruleset「CodeQL merge protection」参与合并判定：CodeQL 对 PR 的分析尚未完成、工具未配置，或 PR 引入 high 及以上安全告警时都会阻止合并（`alerts_threshold = none`、`security_alerts_threshold = high_or_higher`，target 为默认分支）。误报在 Security 标签页 dismiss 并写明理由；需要临时绕过时调整或停用该 ruleset，不要把 CodeQL 加进 required checks。

项目只有一位维护者时，所需批准数为 0，CI 是硬门槛。第二位维护者加入后批准数变为 1，由 CODEOWNERS 负责评审路由。

## 提交信息

使用英文 Conventional Commits：

```text
feat(config): add route validation
fix(sandbox): reject path traversal in ctx.file
docs: describe the release process
```

允许的类型：`feat`、`fix`、`docs`、`refactor`、`perf`、`test`、`build`、`ci`、`chore`、`security`。

建议的 scope：`config`、`match`、`source`、`transform`、`response`、`sandbox`、`runtime-js`、`runtime-python`、`files`、`obs`、`dist`。

破坏性变更使用 `!` 或 `BREAKING CHANGE:` footer。

## 版本号

- 遵循 Semantic Versioning 2.0.0。
- tag 格式为 `vMAJOR.MINOR.PATCH`，例如 `v0.2.0`；**不使用 prerelease 后缀**（见下一条）。
- 版本号与 tag 都不带 `-alpha`/`-beta`/`-rc` 后缀：release-plz 的 tag 探测正则是 `^vX.Y.Z$`（上游硬编码的数字组），带后缀的 tag 它看不见，会让它永远认为仓库已是最新、再也不发版本 PR。pre-1.0 的不稳定语义由 `0.x` 版本、CHANGELOG 文字与 release notes 承担；代价是 cargo-dist 不会把这类 GitHub Release 自动标为 pre-release，需要该标记时应单独在 `release-extras` 里补一步（未做）。
- Cargo 包版本号不带 `v` 前缀。
- 版本号的唯一来源是 workspace 的 `Cargo.toml`。
- 预发布阶段用 `0.x` 版本加 CHANGELOG/release notes 文字表达，不再用版本后缀；`1.0.0` 仍是稳定分界。
- `0.x` minor 版本可以包含破坏性变更；`0.x` patch 版本必须向后兼容。
- `1.0.0` 要求 CLI、配置格式与 `ctx` API version 1 稳定。
- 不要在发布 tag 中使用 build metadata。

## 发布流程

发布由两条 GitHub Actions 工作流串联；版本号的唯一来源仍是 workspace 的 `Cargo.toml`：

1. 把已完成的改动合并进 `main`。这次 push 触发 `.github/workflows/release-plz.yml`。
2. `release-plz release` 根据 `release-plz.toml` 的 `git_only = true` 从 git tag 判断未发布版本；需要发布时创建 tag，并在同一 job 中用 `gh workflow run release.yml -f tag=<tag>` 触发产物流水线。
3. `release-plz release-pr` 按 Conventional Commits 计算下一版本，打开或更新 release PR；PR 包含 `Cargo.toml` 与 `CHANGELOG.md` 改动。只有命中 `release_commits` 的提交才触发版本 PR（`feat`/`fix`/`perf`/`refactor`/`security`/`build`、任意 `deps` scope，或任何带 `!` 的破坏性变更）；`docs`/`ci`/`test`/`chore` 合并不单独发版，它们的条目随下一个真实版本进入 CHANGELOG。
4. 核对 release PR 的版本号、`CHANGELOG.md`、release notes 与 B 层文档的中文译本。release PR 的 commit 由 release-plz 生成、不含 `Signed-off-by`，`dco` 工作流因此对来自本仓库 `release-plz-*` 分支的 PR 跳过检查（跳过的 job 记为 success）；人类提交仍需 `git commit -s`。
5. 合并 release PR；下一次 `release-plz release` 会为合并后的版本创建 tag，并触发产物流水线。
6. `.github/workflows/release.yml` 由 `cargo-dist` 从 `dist-workspace.toml` 生成，只接受 `workflow_dispatch` 的 tag 输入；`.github/release-build-setup.yml` 会在构建前断言 ref 就是 `vMAJOR.MINOR.PATCH` 形式的输入 tag（正则仍接受旧式后缀，供历史 tag 使用），且 tag commit 可从 `origin/main` 到达，绝不直接发布 `main`。它构建 Linux x86_64、macOS arm64、Windows x86_64 的 `.tar.gz`/`.zip`、逐文件 `.sha256`、`sha256.sum`、源码归档与 `types/ctx-api-v1.d.ts`，生成 GitHub artifact attestation，并创建 GitHub Release。
7. `release-extras` post-announce job 在 Release 创建后生成 CycloneDX SBOM、附加 `SHA256SUMS`、构建并推送 `ghcr.io/geeknonerd/stuntdouble:<tag>`（`linux/amd64` 与 `linux/arm64` 共用同一 tag 的镜像索引，arm64 由宿主平台 builder 交叉编译，见 `Dockerfile`）、断言该索引同时覆盖两个平台、附加镜像 digest，并用 `gh attestation verify` 验证已发布的 Linux 二进制与容器 attestation。GHCR tag 已存在时复用 digest，不覆盖、不重新生成 provenance，只验证已有 attestation；存在性检查无法确认时 fail closed。
8. 用“发布验证”中的命令复核 Release；全部资产存在后再公告。

`v0.1.0-alpha.1` 这个旧 tag 对 release-plz 不可见，因此 `0.2.0` 是一次性的桥接版本：版本号与 CHANGELOG 段由人工在同一个 PR 里写好，合并后第 2 步建出 tag `v0.2.0` 并触发产物流水线（已完成）。此后 tag 形如 `v0.2.0` 能被正常识别，第 3 步恢复由 release-plz 打开版本 PR。

`release-extras` 失败或取消时，会把已公开但不完整的 Release 回退为 draft；修复后优先重跑该 job，若需要更换已有产物则发布新的 patch 版本（本项目不使用 prerelease 版本，见「版本号」）。发布失败不得复用或覆盖已有 tag；只有 crates.io 发布损坏时才用 `cargo yank`，绝不删除已发布的版本。

### 发布验证

```bash
tag=v0.2.0
gh release view "$tag" --repo geeknonerd/stuntdouble
gh release download "$tag" --repo geeknonerd/stuntdouble --pattern '*x86_64-unknown-linux-gnu.tar.gz'
gh attestation verify stuntdouble-x86_64-unknown-linux-gnu.tar.gz --repo geeknonerd/stuntdouble
docker pull "ghcr.io/geeknonerd/stuntdouble:${tag}"
docker buildx imagetools inspect "ghcr.io/geeknonerd/stuntdouble:${tag}"
docker buildx imagetools inspect "ghcr.io/geeknonerd/stuntdouble:${tag}" --raw \
  | jq -r '[.manifests[]? | select(.platform.os == "linux") | .platform.architecture] | sort | unique | join(" ")'
# 期望输出：amd64 arm64
```

Release 页面必须列出三个平台的归档、`SHA256SUMS`（同时保留 cargo-dist 的 `sha256.sum`）、`stuntdouble-<version>.cdx.json`、`ctx-api-v1.d.ts`、`stuntdouble-<version>-image.txt`（镜像 tag 与 digest）以及 release notes。

镜像索引必须同时包含 `linux/amd64` 与 `linux/arm64`，`release-extras` 在公告前断言这一点；`v0.2.1` 及更早的 tag 是单平台镜像，重跑它们的 `release-extras` 会在该断言处失败，这是“不覆盖已有产物”的预期结果。

镜像由 `release-extras` 用 job 内 `GITHUB_TOKEN` 推送，因此按 GHCR 默认规则继承运行 workflow 的仓库的可见性与权限模型（公开仓库得到公开包），匿名 `docker pull` 无需登录即可用，也不需要人工确认或设置可见性；public 之后不能改回 private。只有改用 PAT 或 CLI 在 workflow 之外创建**新包名**时才会默认落成 private，那时才需要去 package settings 的 Danger Zone 改一次。依据与核实命令见 [solutions/ci/ghcr-package-visibility-follows-the-publishing-token.md](solutions/ci/ghcr-package-visibility-follows-the-publishing-token.md)。

## 发布产物

每个 release 包含：

- GitHub 生成的源码归档
- Linux x86_64、macOS arm64、Windows x86_64 二进制
- `SHA256SUMS`
- GitHub artifact attestation
- SBOM（CycloneDX 或 SPDX）
- 容器镜像 `ghcr.io/geeknonerd/stuntdouble:<tag>`（`linux/amd64` 与 `linux/arm64`），并公布 digest
- GitHub Release notes
- 每个受支持 `apiVersion` 的 `ctx` API `.d.ts` 类型定义（源码：`types/ctx-api-v1.d.ts`）

只从 tag 构建产物，绝不发布从 `main` 构建的二进制。

签名从 GitHub artifact attestation 开始；只有出现离线验证需求时才引入 Sigstore/cosign。

## Rust 工具链与 MSRV

- `rust-toolchain.toml` 选择 `stable` channel 与所需组件，因此本地构建与 CI 始终使用最新 stable Rust（写作时为 1.98），不固定到某个较旧的版本。
- `Cargo.toml` 声明 `rust-version`，它是 MSRV 下限而不是构建所用工具链；任何不低于该下限的 stable 版本都受支持。
- CI 在 Linux、macOS、Windows 上用 stable 跑主门禁，另有一个 MSRV job 用精确的 `rust-version` 构建，用来捕捉误用新语言特性的情况。
- MSRV 是同时满足依赖树与安全门禁的最低版本，当前为 Rust 1.91（由 Boa 0.22 决定；clap 4.6 与 toml 1.x 要求 1.85）。
- 不得为了保留存在未修复公告的依赖、或依赖无人维护的 crate 而调低 MSRV。Boa 0.20 / 0.21 仍会引入已归档的 `paste` crate，并需要受 RUSTSEC-2026-0009 影响的 `time` 版本，因此这两条线的 `cargo deny check advisories` 会失败；即使 MSRV 更高，无公告的 Boa 版本仍然胜出。
- 提升 MSRV 的依赖升级必须在 PR 中说明代价，并同时更新 `Cargo.toml`、本文件与 `plans/product-definition.md`。
- 默认拒绝 `unsafe`。任何例外都需要注释说明其不变量。

## 本地检查

提交 pull request 前运行：

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo doc --no-deps
cargo deny check
cargo audit
```

非平凡逻辑需要一个最小可运行检查。沙箱、路径穿越与上传处理改动必须有回归测试。

## 依赖

- 优先标准库，其次平台能力，再次已有依赖。
- 新增依赖需要理由、维护状况检查与许可证检查。
- 不允许 GPL 与 AGPL 依赖。
- `cargo-deny` 检查许可证、来源、重复版本与公告。依赖图通过 `deny.toml` 的 `[graph] targets` 限定在发布目标（Linux x86_64、macOS arm64、Windows x86_64），因此不受支持平台的平台专属依赖不会导致许可证门禁失败。
- Dependabot 每周检查 Cargo、Docker 基础镜像与 GitHub Actions。
- 安全修复不得保留给付费层。

## 文档语言与双语结构

文档语言按读者分三层；完整决策见 [ADR 0013](../plans/adr/0013-documentation-language-and-bilingual-structure.md)。

| 层 | 范围 | 语言 |
| --- | --- | --- |
| A | `CONTRIBUTING.md`、`GOVERNANCE.md`、`SECURITY.md`、`CODE_OF_CONDUCT.md`、`CHANGELOG.md`、`.github/` 模板 | 英文单语 |
| B | `README.md`、`docs/index.md`、`docs/guide/**`、`docs/contracts/**`、`demo/README.md` | 英文权威 + `.zh-CN.md` 译本 |
| C | `AGENTS.md`、`CONTEXT.md`、`plans/**`、`research/**`、`docs/development.md`、`docs/agents/**`、`docs/solutions/**` | 中文单语 |

**双语规则**

- 英文文件不带语言后缀，中文译本用 `.zh-CN.md` 后缀放在同一目录。
- 两页都在标题下方放一行语言切换：英文页 `**English** \| [中文](./x.zh-CN.md)`，中文页 `[English](./x.md) \| **中文**`；分隔竖线必须转义，否则 Jekyll/kramdown 会把该行渲染成表格。中文页另有一行“以英文版为准”的声明。
- 英文是唯一权威版本。中文译本允许滞后，但不得与英文矛盾。
- 外部贡献者只提交英文，不因缺少译本被阻塞合并；译本同步由维护者负责。
- 每次发布前核对 B 层中文译本（见发布流程）；补不上就删除对应中文页，不留过时译本。
- 出现第三种语言、B 层超过 10 篇、或引入站点生成器时，迁移到 `docs/<lang>/` 目录布局。
- 新增 B 层页面时，除两个双语文件本身，同一分支还要补齐索引面：根 `README.md` 与 `README.zh-CN.md`、`docs/README.md`、`docs/index.md` 与 `docs/index.zh-CN.md`，以及相邻指南的 `Next steps`/`下一步`。`docs-links` 只验证链接可达与首 8 行的双向语言切换，漏掉的索引它发现不了。

**中文写作约定**

- 领域术语使用 [CONTEXT.md](../CONTEXT.md) 的英文规范词，不造中文译名，也不使用 `_Avoid_` 列出的词。
- 文件名一律英文 kebab-case；标题与正文用中文。
- 错误码、配置键、CLI 参数、路径与代码保留原文（半角），中文正文使用全角标点。
- `docs/solutions/` 的 frontmatter（title、module、tags 等）是机器可读元数据，保留英文；正文用中文。
- 英文页不混入中文，例外只有语言切换链接与 `(Chinese)` 标注。

**校验**

`docs-links` CI 门槛做两件事：`lychee --offline` 检查全部 Markdown 的本地链接；脚本检查每个 `.zh-CN.md` 都有同名英文文件，且两页都含语言切换链接。外链与译文漂移不做自动检查。

本地预检：

```bash
lychee --offline --no-progress --exclude-path target --exclude-path .git './**/*.md'
```

其余约定：公开契约在 `docs/contracts/`；领域词汇在 `CONTEXT.md`，不含实现细节；难以逆转的决策写入 `plans/adr/`。

## 发布自动化配置

- `.github/workflows/ci.yml`：`fmt`、`clippy`、`test`、`docs`、`docs-links`、`deny`、`audit`、`msrv` 八个 job，全部 action 按 SHA 固定。`audit` job 不再经第三方审查 action：直接跑固定版本的 `cargo-audit`，有漏洞即以非 0 退出使 job 失败，也不创建 issue；版本升级需人工改 workflow。原因、实测耗时、运行时核查方法与仍属外部的注解见 [solutions/ci/node20-deprecation-annotations-only-cover-node20-actions.md](solutions/ci/node20-deprecation-annotations-only-cover-node20-actions.md)。
- `.github/workflows/release-plz.yml`：release PR、tag 与 cargo-dist 触发。
- `.github/workflows/release.yml`：由 `dist-workspace.toml` 生成；改配置后运行 `dist generate`，不要手工编辑该文件。
- `.github/workflows/release-extras.yml`：cargo-dist 的 post-announce job，负责 SBOM、GHCR 镜像、digest，以及二进制、容器与必需 Release 资产的验证。
- `.github/release-build-setup.yml`：cargo-dist 注入到每个构建 job 的步骤，拒绝非 tag 或其他 ref 的发布构建。
- `.github/workflows/codeql.yml`：Rust 高级代码扫描，在 `main`、pull request 与每周计划任务上运行；它不加入 required checks，改由 ruleset「CodeQL merge protection」参与合并判定（阈值与误报处置见「Pull request」）。
- 仓库必须允许 GitHub Actions 创建 pull request（Settings → Actions → General → Workflow permissions）。
- crates.io 发布默认关闭（`release-plz.toml` 的 `publish = false`）。启用时把 `publish` 改为 `true`，并在 `release-plz.yml` 的 release job 中提供 `CARGO_REGISTRY_TOKEN`。
- `RELEASE_PLZ_TOKEN`（推荐）：细粒度 PAT，权限为 `Contents: Read and write` 与 `Pull requests: Read and write`。它供 release-plz 创建 tag、分支和 release PR，使 PAT 创建的 release PR 自动触发 CI；PAT 不需要 `Actions: write`，触发 `release.yml` 使用 job 内 `GITHUB_TOKEN` 的 `actions: write`。
- 未配置或 token 失效时，workflow 回退到 `GITHUB_TOKEN`：PR 仍会创建，但其 required checks 可能停在 `Expected` 等待人工批准；批准对应 run，或编辑 PR 触发 `edited` 事件，可恢复 `pr-title` 等检查。
- 更新 PAT 后，用一次由该 PAT 发起的测试 PR 验证 `pull_request` workflows 会自动排队；不要只以 secret 名称存在作为验证。

注意：MSRV（`rust-version`）声明在 [Cargo.toml](../Cargo.toml)，必须等于依赖树中的最高要求；MSRV CI job 用精确的该版本构建并检查锁定的依赖图。

### 已评估的后续硬化项

以下项目已评估，但不在 T9 当前切片处理，按触发条件跟踪：

- [#41](https://github.com/geeknonerd/stuntdouble/issues/41) 发布原子性：当前 `release-extras` 失败时把 Release 回退为 draft；等 cargo-dist 支持完整 draft 编排或项目自管 Release 生命周期后升级。
- [#43](https://github.com/geeknonerd/stuntdouble/issues/43) 必需资产清单单一来源：出现第二个受支持 `apiVersion` 或新资产类型时，从 dist manifest 派生校验清单。
- [#44](https://github.com/geeknonerd/stuntdouble/issues/44) cargo-dist 权限与 installer 摘要：上游提供按 job 权限或摘要校验能力，或项目决定承担 `allow-dirty = ["ci"]` 代价时处理。

## 构建说明

Rust crate 采用 bin+lib 结构：`src/lib.rs` 定义 library，`src/main.rs` 从 `stuntdouble::{config,server}` 导入。该结构让 `cargo test --doc` 与 `cargo doc --no-deps` 能找到 library target。后续新增 crate 时保持这一模式，确保 CI 门禁不会因缺少 library target 而失败。

排障见 [solutions/ci/doctest-lib-target-required.md](solutions/ci/doctest-lib-target-required.md)。

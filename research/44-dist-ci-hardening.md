# Issue #44 调研：cargo-dist CI 权限与安装器校验

- 调研日期：2026-10-19
- 修订：2026-10-20，按 [ADR 0018](../plans/adr/0018-cargo-dist-bootstrap-verification.md) 把 wrapper 设计收敛为内嵌版本与摘要；`cargo-dist-installer.sha256` 只作审计来源，wrapper 不在运行期读取。
- 调研对象：[geeknonerd/stuntdouble#44](https://github.com/geeknonerd/stuntdouble/issues/44)
- 关联决策：[ADR 0018](../plans/adr/0018-cargo-dist-bootstrap-verification.md)
- 证据分层：`[官方声明]`、`[源码事实]`、`[独立测试]`、`[推断]`、`[待验证]`
- 范围：cargo-dist 上游能力、GitHub Actions 官方权限模型、发布供应链标准，以及 #44 可选方案比较

## 结论摘要

1. `[官方声明 + 源码事实]` 截至 2026-10-19，cargo-dist 的最新正式发布版仍是 `v0.33.0`（2026-09-11）。其生成器仍把 `contents: write` 硬编码在工作流顶层，并把 `pull_request` 作为 Release 工作流触发器之一；plan job 因此会继承写权限。
2. `[源码事实 + 独立测试]` `v0.33.0` 仍通过 `curl ... | sh` 自安装 dist；安装器会校验它下载的归档，但不会校验安装器脚本自身。`sha256.sum` 不包含 `cargo-dist-installer.sh`，GitHub attestation 也没有覆盖该安装器脚本。
3. `[源码事实]` `allow-dirty = ["ci"]` 会让 dist 停止更新和检查 `release.yml`。官方明确建议避免使用，并警告升级 dist 等流程可能失效。
4. `[官方声明 + 源码事实]` `github-custom-job-permissions` 只覆盖自定义 job；内置 plan/host 等 job 的权限仍由生成器硬编码。相关上游 PR/issue（#2387、#2279、#2420）尚未落地为正式版本能力。
5. `[推断]` #44 的第一个触发条件（上游原生 per-job permissions 或 checksum-pinned installer）仍未满足；上游 #2387、#2420 都还没有合并。
6. `[独立测试]` 存在一条无需 `allow-dirty` 的原生替代路径：`pr-run-mode = "skip"` 让生成工作流不再含 `pull_request`；`cargo-dist-url-override` 可把自举脚本 URL 改到仓库内固定 commit 的 bootstrap wrapper。最终设计让两个 wrapper 内嵌 `cargo-dist` 版本与 installer SHA-256，只从 workspace 读取 `cargo-dist-version` 做断言；摘要清单仅作审计。本地 `dist 0.33.0` 实验确认生成结果中的 shell 命令变为该 URL，Windows 命令变为 `irm .../cargo-dist-installer.ps1 | iex`，且 `dist generate --check` 通过。
7. `[推断]` 若采用上一条路径，`#44` 的三条验收标准都可以在不手改 `release.yml` 的前提下满足；代价是引入并维护 bootstrap wrapper 与摘要清单，且 `cargo-dist-url-override` 是上游用于快速迭代的配置，不在 v1 配置参考的重点文档中。
8. `[推断]` 若不接受 wrapper 维护成本，则仍只剩两条路：继续等待上游，或显式接受 `allow-dirty = ["ci"]` 的维护成本后手改生成工作流。
9. `[独立测试]` cargo-dist `v0.33.0` 的 Linux 归档有可验证 attestation，且可用 `gh attestation verify` 约束签名工作流与来源 ref；安装器脚本没有 attestation。因此“下载归档并验证”比“继续执行安装器脚本”更容易给出可验证边界。

## 1. #44 现状与触发条件

### 1.1 issue 原文事实

`[官方声明]` #44 由 `geeknonerd` 于 2026-09-22 创建，状态 `OPEN`，标签为 `security`、`area:distribution`，无评论。原文核心事实：

- cargo-dist 生成的 `.github/workflows/release.yml` 在顶层设置 `contents: write`，该权限也应用到 `pull_request` plan job。
- dist 通过 `curl ... | sh` 安装，没有校验下载安装器的摘要。
- fork PR token 是只读的，因此当前被视为“已接受的风险”，但它削弱了最小权限与发布可复现性。

来源：[#44](https://github.com/geeknonerd/stuntdouble/issues/44)（核验日期 2026-10-19）

### 1.2 当前仓库中的对应事实

`[源码事实]` 本地仓库在 2026-10-19 的状态：

- `.github/workflows/release.yml:17-18`：工作流顶层为 `contents: write`。
- `.github/workflows/release.yml:42`：触发器中包含 `pull_request`。
- `.github/workflows/release.yml:71`：plan job 使用未校验的 `cargo-dist-installer.sh | sh`。
- `.github/workflows/release.yml:119-122`：`build-local-artifacts` 已有 job 级 `attestations: write`、`contents: read`、`id-token: write`。
- `.github/workflows/release.yml:311-315`：自定义 `release-extras` job 已有 job 级权限。
- `.github/workflows/release.yml:256-300` 与 `318+`：`host`、`announce` 没有自己的 `permissions` 块，继续继承顶层 `contents: write`。
- `.github/workflows/ci.yml:9`：常规 CI 顶层为 `contents: read`。
- `.github/workflows/ci.yml:95-100`：`release-drift` 也用 `curl ... | sh` 安装 dist。
- `dist-workspace.toml:7`：`cargo-dist-version = "0.33.0"`。
- `dist-workspace.toml:32`：`github-attestations = true`。
- `dist-workspace.toml:36`：已使用 `github-custom-job-permissions` 给 `release-extras` 授权。

### 1.3 触发条件判定

| #44 触发条件 | 2026-10-19 判定 | 依据 |
| --- | --- | --- |
| 上游提供 per-job permissions，且无需手改生成 CI | 未满足 | `v0.33.0` 仍硬编码顶层 `contents: write`；`github-custom-job-permissions` 只覆盖自定义 job。 |
| 上游提供 checksum-pinned installer | 未满足 | `v0.33.0` 仍生成 `curl \| sh`；#2420 仍 open。 |
| 项目接受 `allow-dirty = ["ci"]` 的手改维护成本 | 由项目决策 | 官方文档允许，但明确不建议，并说明升级/重新生成流程会受影响。 |

## 2. cargo-dist 一手事实

### 2.1 版本与仓库状态

`[官方声明]` GitHub Releases 最新正式版为 `v0.33.0`，发布于 2026-09-11，release target commit 为 `a1c829ffacb7ce98fb617b7acd7ffdd2282b45c2`。

- Release：<https://github.com/axodotdev/cargo-dist/releases/tag/v0.33.0>
- API 核验：<https://api.github.com/repos/axodotdev/cargo-dist/releases/latest>（核验日期 2026-10-19）

`[官方声明]` 仓库还存在旧 prerelease tag `v1.0.0-rc.1`（2025-01-09），但其 commit 落后于当前主线，不是“最新正式版”。不应把它当作高于 `0.33.0` 的可用版本。

- Release：<https://github.com/axodotdev/cargo-dist/releases/tag/v1.0.0-rc.1>
- Commit：<https://github.com/axodotdev/cargo-dist/commit/8580b6ea881b8bd1d3a8db79d643b75ba125604e>

`[官方声明]` 上游 issue #2506（2026-09-12 创建，open）说明：由于 `axoproject` crate 的权限问题，dist `0.33.0` 当时未能发布到 crates.io。

- Issue：<https://github.com/axodotdev/cargo-dist/issues/2506>
- `[待验证]` 本次 crates.io API 请求返回 403，未能独立复核 2026-10-19 当日的 crates.io 版本状态；交付结论以上游 issue 为准。

`[源码事实]` `main` 在 2026-10-19 的 HEAD 为 `d1055f48071e3b253896e85ed0bcb8064af50512`（2026-09-04，minijinja 依赖升级）。相对 `v0.33.0` 的 15 个 commit 主要是依赖升级，没有落地 #44 所需能力。

- HEAD：<https://github.com/axodotdev/cargo-dist/commit/d1055f48071e3b253896e85ed0bcb8064af50512>
- 主源码中仍可见：<https://github.com/axodotdev/cargo-dist/blob/main/cargo-dist/src/backend/ci/github.rs#L306-L310>
- 主源码 installer 仍可见：<https://github.com/axodotdev/cargo-dist/blob/main/cargo-dist/src/backend/ci/mod.rs#L123-L133>

### 2.2 权限模型

`[源码事实]` `v0.33.0` 生成 GitHub CI 时直接构造 root permissions，并硬编码 `contents: write`：

```rust
let mut root_permissions = GithubPermissionMap::new();
root_permissions.insert("contents".to_owned(), GithubPermission::Write);
```

- 源码：<https://github.com/axodotdev/cargo-dist/blob/v0.33.0/cargo-dist/src/backend/ci/github.rs#L306-L310>
- 模板：<https://github.com/axodotdev/cargo-dist/blob/v0.33.0/cargo-dist/templates/ci/github/release.yml.j2#L26-L31>
- PR 触发器模板：<https://github.com/axodotdev/cargo-dist/blob/v0.33.0/cargo-dist/templates/ci/github/release.yml.j2#L65-L68>

`[官方声明 + 源码事实]` `github-custom-job-permissions` 自 `0.18.0` 起存在，但只允许为自定义 job 覆盖权限。默认所有自定义 `publish-jobs` 获得 `{ id-token = "write", packages = "write" }`；其他自定义 job 默认无特殊权限。

- 文档：<https://github.com/axodotdev/cargo-dist/blob/v0.33.0/book/src/reference/config.md#L1498-L1515>
- 引入 PR #1179：<https://github.com/axodotdev/cargo-dist/pull/1179>（closed，2024-07-03）
- 相关 issue #909：<https://github.com/axodotdev/cargo-dist/issues/909>（closed；维护者回复确认该能力已落地，但仅解决自定义 job 权限）

`[官方声明]` issue #2279 “Permission to specific hosting job?” 仍 open（2026-02-09 创建）。正文描述的是用户手改 `host` job 权限的 workaround，而不是已发布的内置 job 权限配置能力。

- <https://github.com/axodotdev/cargo-dist/issues/2279>

`[官方声明 + 源码事实]` PR #2387（2026-05-03 创建，open，当前 `CONFLICTING`）明确把顶层权限从 `contents: write` 改为 `contents: read`，并把写权限下放到需要发布的内置 job。这正好覆盖 #44 的权限部分，但截至 2026-10-19 未合并、未发布。

- <https://github.com/axodotdev/cargo-dist/pull/2387>
- Diff 中 `root_permissions` 改为 `Read`，并为 `host`/`announce` 增加 job 级 `contents: write`。
- `[官方声明]` 维护者在 2026-05-22 的评论中表示不认为 tag shell injection 本身是安全问题，并询问 `contents: read` 是否实测可用；贡献者 2026-05-28 回复“已验证可用”，并愿意拆出权限变更。之后没有合并记录。

`[官方声明]` issue #2133 / PR #2134 已把 attestation 相关权限从顶层移到对应 job，但顶层 `contents: write` 仍然保留；该修复不等价于 #44 的完整权限收敛。

- issue：<https://github.com/axodotdev/cargo-dist/issues/2133>
- PR：<https://github.com/axodotdev/cargo-dist/pull/2134>

### 2.3 installer 下载与校验

`[源码事实]` `v0.33.0` 的 shell 自安装命令为：

```sh
curl --proto '=https' --tlsv1.2 -LsSf <installer-url>/<installer-name>.sh | sh
```

- <https://github.com/axodotdev/cargo-dist/blob/v0.33.0/cargo-dist/src/backend/ci/mod.rs#L123-L146>

`[源码事实]` 安装器会对自己下载的归档做摘要校验；摘要值在生成安装器时嵌入。校验函数支持 sha256/sha512/sha3/blake2s 等，但脚本没有校验“安装器自身”。

- 嵌入摘要：<https://github.com/axodotdev/cargo-dist/blob/v0.33.0/cargo-dist/templates/installer/installer.sh.j2#L233-L305>
- 校验实现：<https://github.com/axodotdev/cargo-dist/blob/v0.33.0/cargo-dist/templates/installer/installer.sh.j2#L1617-L1655>
- `[官方声明]` 官方 checksum 文档承认：HTTPS + 未签名 checksum 的安全收益很小，主要能发现数据损坏。
  - <https://github.com/axodotdev/cargo-dist/blob/v0.33.0/book/src/artifacts/checksums.md#L1-L19>

`[独立测试]` 2026-10-19 对 `v0.33.0` release 资产做以下核验：

- `cargo-dist-installer.sh` 大小 `58085`，SHA-256 为 `c5b53ac2559eb26e3b3dc28755eb5d011ed6ec9ac97fa4fc2a2459872e766b5e`。
- GitHub API 返回的 asset digest 与本地计算一致。
- `sha256.sum` 只有 9 个归档/source 条目，不包含 `cargo-dist-installer.sh`。
- `gh attestation verify /tmp/cargo-dist-installer-v0.33.0.sh --repo axodotdev/cargo-dist` 返回 404，即安装器脚本没有可用 attestation。

`[独立测试]` 同一 release 的 Linux 归档 `cargo-dist-x86_64-unknown-linux-gnu.tar.xz`：

- 本地 SHA-256 为 `4b3f0a5f0ebbdb798f6db649d01b32ba1518376b6f7a0502b7d92b75cc2c8293`，与 `sha256.sum` 及 GitHub API digest 一致。
- `gh attestation verify ... --repo axodotdev/cargo-dist` 通过。
- 进一步约束签名工作流和来源 ref 也通过：

```sh
gh attestation verify <archive> \
  --repo axodotdev/cargo-dist \
  --signer-workflow axodotdev/cargo-dist/.github/workflows/release.yml \
  --source-ref refs/tags/v0.33.0
```

`[官方声明]` issue #2420（2026-06-03 创建，open，无评论）请求为“dist 自安装”提供 checksum 校验，提出两种变体：固定安装器 SHA-256；或提供 opt-in 的 `cargo install --locked --version` 自举方式。

- <https://github.com/axodotdev/cargo-dist/issues/2420>

`[官方声明]` PR #2352（2026-04-02 创建，open）尝试在预编译二进制不可用时 fallback 到 `cargo install --locked --version <version>`，但维护者指出当前实现会吞掉任意 `curl | sh` 失败，后续评论建议只在没有预编译二进制的 host 上使用 source build。

- <https://github.com/axodotdev/cargo-dist/pull/2352>

`[官方声明]` 其他相关但不等价的条目：

- #439：`do checksum integrity check in installers`，open，2023-09-20。针对输出安装器校验下载归档。
- PR #1497：`Verify checksums in install.sh`，closed/merged，2024-10-28 至 2024-11-06。针对 shell 安装器的归档校验。
- #2497：`Verify archive checksums in PowerShell installers`，open，2026-09-03。针对 PowerShell 安装器的归档校验，不是自安装脚本。
- #2339：安装器脚本在 GitHub artifact 传输中损坏的问题，closed，2026-03-27。与自安装供应链不同。

### 2.4 `allow-dirty = ["ci"]` 的语义与代价

`[官方声明]` 官方配置文档：

- `allow-dirty` 是 `generate` task 的忽略列表。
- `"ci"` 表示“don't check/regenerate ci scripts (release.yml)”。
- 官方建议避免设置，因为它会阻止 dist 在配置变化时更新这些文件。

来源：<https://github.com/axodotdev/cargo-dist/blob/v0.33.0/book/src/reference/config.md#L141-L161>

`[官方声明]` 官方 CI 定制文档进一步说明：

- 设置 `allow-dirty = ["ci"]` 后，dist 停止更新 `release.yml`，也停止检查它是否过期。
- 会失去大量便利流程；“Even things as simple as updating dist will stop working.”
- 未来 dist 版本可能完全改变 CI 结构，不能保证手改文件能平滑升级。

来源：<https://github.com/axodotdev/cargo-dist/blob/v0.33.0/book/src/ci/customizing.md#L340-L362>

`[源码事实]` 生成器源码对 `allow_dirty` 的处理是跳过对应生成/检查 task。

来源：<https://github.com/axodotdev/cargo-dist/blob/v0.33.0/cargo-dist/src/lib.rs#L869-L905>

`[推断]` 一旦启用该选项，`dist generate --check` 将不再校验 `release.yml` 的漂移。若仍要保证可复现，需要在 `release-drift` 中增加独立校验步骤，例如：在临时目录重新生成上游 `release.yml`，再应用确定性的补丁，最后与仓库内受控版本比较。

### 2.5 `pr-run-mode` 的可选语义

`[官方声明]` `pr-run-mode` 控制 Release CI 在 PR 上运行到什么程度：

- `"skip"`：不在 PR 中检查发布流程，也不生成 `pull_request` 触发器。
- `"plan"`：在 PR 上运行 `dist plan`（默认、推荐）。
- `"upload"`：在 PR 上构建并上传 artifacts（昂贵）。

来源：<https://github.com/axodotdev/cargo-dist/blob/v0.33.0/book/src/reference/config.md#L1420-L1440>

`[源码事实]` 模板只在 `pr_run_mode != "skip"` 时输出 `pull_request:`。

来源：<https://github.com/axodotdev/cargo-dist/blob/v0.33.0/cargo-dist/templates/ci/github/release.yml.j2#L65-L68>

`[推断]` 这是无须 `allow-dirty` 就能把 Release 工作流从 PR 触发中移除的原生配置能力。代价是失去内置 PR plan；可以另建一个手写、只读、只做 `dist plan` 的 PR workflow 来补回覆盖。

### 2.6 `cargo-dist-url-override` 作为无漂移自举入口

`[官方声明]` PR #1509 引入了 `cargo-dist-url-override`，目的是“quick iteration on cargo-dist itself”；上游集成测试也用该配置验证生成结果。配置说明要求目标 URL 具有与 `./target/distrib` 类似的结构，安装器文件名固定为 `cargo-dist-installer.sh` / `cargo-dist-installer.ps1`。

来源：

- <https://github.com/axodotdev/cargo-dist/pull/1509>
- <https://github.com/axodotdev/cargo-dist/blob/v0.33.0/cargo-dist/tests/integration-tests.rs#L2142-L2150>
- <https://github.com/axodotdev/cargo-dist/blob/v0.33.0/cargo-dist/src/config/v0.rs#L22-L28>

`[源码事实]` 生成器把该 URL 视为 dist 自安装策略的更高优先级入口：shell 生成 `curl <override>/cargo-dist-installer.sh | sh`，PowerShell 生成 `irm <override>/cargo-dist-installer.ps1 | iex`；该分支位于按 `cargo-dist-version` 计算上游 release URL 之前。

来源：<https://github.com/axodotdev/cargo-dist/blob/v0.33.0/cargo-dist/src/backend/ci/mod.rs#L88-L146>

`[独立测试]` 在本地隔离克隆中，用已安装的 `dist 0.33.0` 做了以下实验，没有启用 `allow-dirty`：

- 在 `dist-workspace.toml` 增加 `cargo-dist-url-override = ...` 与 `pr-run-mode = "skip"`。
- `dist generate` 成功，生成的 plan job 命令变为 `curl ... <override>/cargo-dist-installer.sh | sh`；`dist plan` 的 Windows matrix 命令变为 `irm ... <override>/cargo-dist-installer.ps1 | iex`。
- `dist generate --check` 返回 0；生成的 Release 工作流不再含 `pull_request` 触发器。

`[推断]` 这提供了一条“不改生成文件、不启用 `allow-dirty`”的注入点：把 override 指向仓库内 commit-pinned 的 bootstrap wrapper，wrapper 按内嵌版本下载并 SHA-256 校验上游 installer，最后执行。版本和摘要都写进 wrapper，因此 pin 同时固定代码与期望值；升级 cargo-dist 时更新 wrapper 内嵌常量与审计清单，不需要重放 `release.yml` 补丁。

`[待验证]` `cargo-dist-url-override` 不是面向供应链安全的官方保证，官方将其定位为快速迭代配置；wrapper 的 shell/PowerShell 实现在 macOS、Windows runner 上仍需由 CI 矩阵持续验证，wrapper 代码变更则必须按 ADR 0018 的两阶段流程更新 commit pin。

## 3. GitHub 官方权限与安全事实

### 3.1 fork PR 与同仓库 PR 的 `GITHUB_TOKEN` 差异

`[官方声明]` `GITHUB_TOKEN` 权限计算顺序：

1. 先取 enterprise/org/repo 默认值。
2. 再应用 workflow 顶层 `permissions`。
3. 再应用 job 级 `permissions`。
4. 如果 workflow 由 `pull_request`（不是 `pull_request_target`）触发且来自 forked repository，并且未启用 “Send write tokens to workflows from pull requests”，则所有 write 权限被降为 read。

来源：

- <https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax#permissions>
- 源码：<https://github.com/github/docs/blob/main/content/actions/reference/workflows-and-actions/workflow-syntax.md#L352-L366>

`[官方声明]` fork PR 默认策略：“Run workflows from fork pull requests ... using a `GITHUB_TOKEN` with read-only permission, and with no access to secrets.”

来源：<https://github.com/github/docs/blob/main/data/reusables/actions/private-repository-forks-options.md>

`[官方声明]` 对 fork，`permissions` 通常只能增加/移除 read，不能授予 write；唯一例外是管理员启用 “Send write tokens to workflows from pull requests”。

来源：<https://github.com/github/docs/blob/main/data/reusables/actions/forked-write-permission.md>

`[推断]` 同仓库分支 PR 不满足“来自 forked repository”的降权条件，因此不会触发上述 write→read 调整。结合当前 `release.yml` 顶层 `contents: write`，`plan` job 在同仓库 PR 上会获得写权限。该结论由官方规则推导，未在本仓库运行恶意 PR 做实测。

### 3.2 job 级权限与最小权限

`[官方声明]` 可以在 job 内设置 `permissions`，只给该 job 所需权限。若在 `permissions` 中指定了任一权限，所有未列出的权限会被设为 `none`。

来源：

- <https://github.com/github/docs/blob/main/data/reusables/actions/jobs/section-assigning-permissions-to-jobs-specific.md>
- <https://github.com/github/docs/blob/main/data/reusables/actions/github-token-available-permissions.md>

`[官方声明]` 官方安全建议：默认把 `GITHUB_TOKEN` 对 repository contents 的权限设为只读，再在单个 job 中按需提升。

来源：<https://github.com/github/docs/blob/main/content/actions/reference/security/secure-use.md#L32-L34>

`[官方声明]` 对第三方 action/workflow：

- 最佳措施是使用 workflow dependency lockfile。
- 无法使用 lockfile 时，固定到 full-length commit SHA，并审计 action 源码。
- 只固定 tag 仍可能因 tag 被移动或仓库被盗而失效。

来源：<https://github.com/github/docs/blob/main/content/actions/reference/security/secure-use.md#L129-L160>

### 3.3 `pull_request_target` 的风险

`[官方声明]` `pull_request_target` 运行在 elevated trust 下：job 获得 base repository 的 `GITHUB_TOKEN` 和 repository/organization secrets。默认情况下 workflow 与 checkout 来自 base 默认分支，因此安全；一旦把 checkout 指向 fork 的 head/merge commit 并执行其中代码，就会形成 “pwn request”。

- <https://github.com/github/docs/blob/main/content/actions/reference/security/securely-using-pull_request_target.md#L16-L75>
- <https://docs.github.com/en/actions/reference/security/securely-using-pull_request_target>

`[官方声明]` 官方建议：不需要时避免 `pull_request_target`；需要特权上下文时优先考虑分离工作流，并确保不可信 PR 代码不被 checkout/build/run。

来源：<https://github.com/github/docs/blob/main/content/actions/reference/security/secure-use.md#L109-L127>

`[官方声明]` GitHub 对 public repository 的 `pull_request_target` 默认策略正在 evaluate，计划于 2026-11-02 强制执行；这会进一步降低该 trigger 的可用性。

来源：<https://github.com/github/docs/blob/main/content/actions/reference/security/securely-using-pull_request_target.md#L77-L107>

`[推断]` 为解决 #44 而引入 `pull_request_target` 会扩大信任面，且与官方建议相反；不应作为方案。

## 4. 发布供应链最佳实践

### 4.1 SLSA v1.2

`[官方声明]` SLSA 当前规范页为 v1.2。验证 artifact 时，规范建议：

- 确认 builder identity 在可信 builder 列表中，并映射到允许的 SLSA level。
- 验证 provenance envelope 的签名。
- 校验 provenance 的 `subject` 与 artifact digest 匹配。
- 校验 `buildType`、`externalParameters` 等期望值；未识别的 external parameters 应导致失败。

来源：<https://slsa.dev/spec/v1.2/verifying-artifacts>（核验日期 2026-10-19）

`[官方声明]` SLSA Build L3 对 provenance 的要求包括：

- provenance 对 tenant 不可伪造。
- 用于认证 provenance 的密钥必须只对 build service account 可访问，不能对用户自定义 build steps 可访问。
- provenance 的每个字段必须由可信 control plane 生成或验证，用户控制的 build steps 不能注入或篡改。

来源：<https://slsa.dev/spec/v1.2/build-requirements>（核验日期 2026-10-19）

`[推断]` 把构建工具本身纳入“被验证的 artifact”很重要：SLSA 关注 artifact 的 builder 身份与 provenance 完整性；如果构建工具通过未校验的脚本引导，则验证链在 builder 之外又多了一个不可验证入口。

### 4.2 OpenSSF Scorecard

`[官方声明]` `Token-Permissions`：

- 检查自动化 workflow token 是否遵循最小权限。
- 满分要求顶层 permissions 为只读，所需 write 在 run/job level 声明。
- 若所有 job 都有权限声明但顶层未声明，会扣 1 分，因为未来新增 job 可能漏配。

来源：<https://github.com/ossf/scorecard/blob/main/docs/checks.md#L659-L710>（核验日期 2026-10-19）

`[官方声明]` `Dangerous-Workflow`：

- 检测 `pull_request_target`/`workflow_run` 与显式 PR checkout 组合形成的不可信代码 checkout。
- 检测不可信上下文变量进入 inline script 造成的脚本注入。

来源：<https://github.com/ossf/scorecard/blob/main/docs/checks.md#L284-L310>

`[官方声明]` `Pinned-Dependencies`：

- 构建和发布流程中的 GitHub workflow、shell script、Dockerfile 依赖应按 hash 固定。
- 对 GitHub workflows，应 pin dependencies by hash。

来源：<https://github.com/ossf/scorecard/blob/main/docs/checks.md#L480-L530>

### 4.3 NIST SSDF SP 800-218

`[官方声明]` 与 #44 最直接相关的 SSDF 实践：

- `PO.3.2`：部署、运行和维护工具/工具链时遵循安全实践；示例包括实现可复现构建、定期验证每个工具的完整性和 provenance。
- `PO.5.1`：分离并保护开发、构建、测试、分发环境；最小化对 build service 的直接人工访问和网络暴露。
- `PS.1.1`：按最小权限存储所有形式的代码，包括 configuration-as-code；示例包括用密码学哈希保护文件完整性。
- `PS.2.1`：向软件获取方提供软件完整性验证信息；示例包括在安全网站发布 release file 的 cryptographic hashes。
- `PS.3.1`/`PS.3.2`：安全归档 release、完整性验证信息和 provenance，并让接收方能验证 provenance 完整性。
- `PW.4.1` 示例 8：如果无法确认所获取二进制的完整性或 provenance，则在验证源码完整性和 provenance 后从源码构建。

来源：<https://nvlpubs.nist.gov/nistpubs/SpecialPublications/NIST.SP.800-218.pdf>（核验日期 2026-10-19；DOI: <https://doi.org/10.6028/NIST.SP.800-218>）

### 4.4 Sigstore / cosign

`[官方声明]` 对 blob 的 keyless 验证可以用 bundle 中的 certificate 和 signature，并约束身份与 OIDC issuer：

```sh
cosign verify-blob <file> --bundle artifact.sigstore.json \
  --certificate-identity=name@example.com \
  --certificate-oidc-issuer=https://accounts.example.com
```

来源：<https://docs.sigstore.dev/cosign/verifying/verify/>（核验日期 2026-10-19）

`[推断]` Sigstore/cosign 适合“发布方签名，消费方按预期身份验证”的模型；若 cargo-dist 不提供安装器签名，则消费方无法仅凭 cosign 验证上游安装器脚本。

### 4.5 GitHub Artifact Attestations

`[官方声明]` 为二进制生成 GitHub artifact attestation 需要：

```yaml
permissions:
  id-token: write
  contents: read
  attestations: write
```

验证命令：

```sh
gh attestation verify PATH/TO/ARTIFACT -R ORG/REPO
```

来源：<https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations/use-artifact-attestations>

`[官方声明]` GitHub 的 artifact attestations 本身提供 SLSA v1.0 Build Level 2；配合 reusable workflow 可达到 Build Level 3。官方同时警告：attestation 不是 artifact 安全的保证，只建立来源与构建说明的链接。

- <https://github.com/github/docs/blob/main/content/actions/concepts/security/artifact-attestations.md#L9-L45>
- <https://github.com/github/docs/blob/main/content/actions/how-tos/secure-your-work/use-artifact-attestations/increase-security-rating.md>

`[独立测试]` 在 2026-10-19 的测试中，cargo-dist 的 Linux 归档 attestation 可通过，且可约束：

- `--repo axodotdev/cargo-dist`
- `--signer-workflow axodotdev/cargo-dist/.github/workflows/release.yml`
- `--source-ref refs/tags/v0.33.0`

同一测试中，`cargo-dist-installer.sh` 的 attestation lookup 返回 404。`[推断]` 这说明“直接验证被执行的安装器脚本”在当前上游发布物中不可行，但“验证随后执行的归档”可行。

## 5. 可选方案比较

假设目标是同时满足 #44 的三条验收标准：PR 无发布写权限；dist 安装器按固定摘要校验；生成/更新流程可复现且有文档。

| 方案 | 安全性 | 可复现性 | 维护成本 | 对 cargo-dist 升级的影响 | 能否满足 #44 | 评价 |
| --- | --- | --- | --- | --- | --- | --- |
| A. 等待上游 | 取决于上游 | 取决于上游 | 0，现状继续暴露 | 无 | 当前不能满足 | #2387、#2420 未落地；只能作为并行跟踪 |
| B. 仅启用 `allow-dirty = ["ci"]` 并手改 `release.yml` | 可达到高 | 取决于补丁+校验流程 | 高 | 每次升级需重放补丁；`dist generate --check` 不再覆盖 `release.yml` | 可满足，但必须补齐流程文档 | 最直接，但维护成本真实存在 |
| C. 把 PR dry-run 拆出低权限 workflow | 高（PR 面） | 高 | 中 | `pr-run-mode = "skip"` 是原生配置，影响小 | 只解决权限部分 | 建议与 E/G 组合 |
| D. 自定义 publish job | 中 | 中 | 中 | custom job 权限可控 | 不能解决内置 plan/host 顶层权限与安装器 | 只能作为发布扩展，不是 #44 主解 |
| E. 下载归档并校验摘要/attestation | 高 | 高 | 中 | 仍需手改生成文件或另建 workflow | 可解决安装器部分 | 已验证归档 attestation 可用，优于执行脚本 |
| F. 从源码安装 | 中高 | 高（可用 commit/tag+`--locked`） | 中高（构建耗时） | 不依赖 installer 资产；但需改生成 CI 或另建 workflow | 可解决安装器部分 | 适合作为 fallback，不适合作为默认快速路径 |
| G. `cargo-dist-url-override` + 固定 bootstrap wrapper | 高 | 高 | 中 | 只改配置并重新生成，不需要补丁重放 | 可满足 | 已验证生成链路；wrapper 必须来自不可变 ref，并维护内嵌版本/摘要与审计清单 |

### 方案 A：等待上游

优点：不引入手改，不增加维护负担。

缺点：当前不能消除顶层写权限，也不能校验 installer。跟踪项：

- #2387：已提出 root read + job write，open、冲突，未合并。
- #2420：installer checksum，open、无评论。
- #2279：内置 hosting job 权限，open。

### 方案 B：`allow-dirty = ["ci"]` + 手改

建议只改两处语义：

1. 顶层 `permissions` 改为 `contents: read`，仅 `host`、`announce`、`release-extras` 等需要发布的 job 提升权限。
2. 把每个 `curl ... | sh` 替换为“固定版本归档下载 + 摘要/attestation 校验 + 解压后使用”。

关键维护要求：

- 用 `pr-run-mode = "skip"` 让生成工作流不再含 `pull_request`，减少手改后暴露面。
- 在 `release-drift` 中新增“重新生成上游文件 → 应用确定性补丁 → 与仓库版本比较”的步骤；不能只依赖 `dist generate --check`，因为 `allow-dirty` 会跳过该文件的检查。
- 在文档中记录升级 dist 时如何重放补丁、如何验证归档 attestation、以及何时可以撤销 `allow-dirty`（上游能力落地后）。

### 方案 C：拆分 PR dry-run

原生配置步骤最小：

```toml
[dist]
pr-run-mode = "skip"
```

然后新增一个手写 PR workflow，例如只做：

- `actions/checkout`，`persist-credentials: false`
- 顶层 `permissions: contents: read`
- 用固定 commit SHA 的第三方 action
- 用固定版本 + 校验后的方式安装 dist
- 运行 `dist plan`

优点：不需要为了“PR 只读”而手改 `release.yml`，且 Release 工作流只在 `workflow_dispatch` 下运行。

限制：单独的 PR workflow 仍需要自己的 dist 安装方案；它不能替代 Release 工作流中的安装器校验。

### 方案 D：自定义 publish job

当前仓库已经有 `release-extras` 自定义发布 job，并已配置 `github-custom-job-permissions`。这证明自定义 job 权限模型可用，但它不改变内置 plan/host 的权限继承，也不替换 plan/build 的 dist 自安装。因此它不能单独关闭 #44。

### 方案 E：下载归档并校验摘要/attestation

比继续执行 `cargo-dist-installer.sh` 更可验证：

- 归档在 `sha256.sum` 中，且有 GitHub attestation。
- 安装器脚本既不在 `sha256.sum` 中，也没有 attestation。

两种可落地的校验方式：

1. 固定版本 + 固定 SHA-256：

   ```sh
   curl --proto '=https' --tlsv1.2 -LsSf \
     https://github.com/axodotdev/cargo-dist/releases/download/v0.33.0/cargo-dist-x86_64-unknown-linux-gnu.tar.xz \
     -o dist.tar.xz
   echo "4b3f0a5f0ebbdb798f6db649d01b32ba1518376b6f7a0502b7d92b75cc2c8293  dist.tar.xz" \
     | sha256sum --check
   ```

2. 固定版本 + GitHub attestation，并约束签名 workflow 与 source ref：

   ```sh
   gh attestation verify dist.tar.xz \
     --repo axodotdev/cargo-dist \
     --signer-workflow axodotdev/cargo-dist/.github/workflows/release.yml \
     --source-ref refs/tags/v0.33.0
   ```

`[推断]` 方式 2 的信任根是 GitHub/Sigstore 与上游签名工作流，而不是静态 hash；方式 1 的信任根是仓库内审查过的固定摘要。两者都可以进入受控 `release.yml`，但方式 2 对“上游是否真的构建了该归档”的表达更强。

### 方案 F：从源码安装

可选命令形式：

```sh
cargo install --locked --git https://github.com/axodotdev/cargo-dist \
  --rev a1c829ffacb7ce98fb617b7acd7ffdd2282b45c2 \
  cargo-dist
```

优点：不依赖 GitHub Release 的 installer 脚本；固定 commit 和 `--locked` 提供较强的可复现边界。

缺点：需要 Rust 工具链和编译时间；对 PR 计划和发布 job 都会增加耗时；仍需修改生成 CI 或新建 workflow。它符合 NIST SSDF `PW.4.1` 示例 8 的“无法确认二进制完整性/provenance 时从源码构建”思路，但工程成本高于下载并验证归档。

### 方案 G：`cargo-dist-url-override` + 固定 bootstrap wrapper

这是当前最接近 #44 验收标准、又不启用 `allow-dirty` 的方案：

1. 在 `dist-workspace.toml` 设置 `pr-run-mode = "skip"`，让 Release 工作流不再有 `pull_request` 触发器。
2. 在仓库内增加 `cargo-dist-installer.sh`、`cargo-dist-installer.ps1` 与版本/摘要清单；wrapper 内嵌目标版本与两个 installer 摘要，只从 workspace 读取 `cargo-dist-version` 做断言，下载对应上游 installer、校验 SHA-256 后再执行。
3. 用 `cargo-dist-url-override` 指向不可变 ref（优先 full commit SHA；至少是受保护且不可移动的 tag）下的 bootstrap 目录。
4. `release-drift` 改用仓库内 wrapper 安装 dist，并由检查脚本核对 wrapper、manifest、workspace 版本、pin 内容与 `dist plan` installer 路由；`dist generate --check` 继续负责生成文件对账。

优点：

- 不需要 `allow-dirty`，也不手改 `release.yml`。
- PR 不再触发 Release 工作流，安装器又被摘要固定，能覆盖 #44 的三条验收标准。
- 与现有 `dist.github-action-commits` 的“按不可变 commit 固定第三方依赖”思路一致。

代价与约束：

- `cargo-dist-url-override` 官方定位是快速迭代配置，不是 security API；使用前应把它写进 ADR 或在 `docs/development.md` 记录风险与撤销条件。
- wrapper 自身不是被运行时摘要校验，而是靠不可变 ref 提供完整性；若 ref 可移动，整个方案退化。
- wrapper 内嵌的版本/摘要与审计清单必须随 `cargo-dist-version` 更新，否则会安装旧 dist 或直接失败；需要增加一致性 CI 检查。
- macOS/Windows 的 SHA-256 工具不同（`sha256sum`、`shasum -a 256`、`Get-FileHash`），必须在发布矩阵上实测。

## 6. 建议

`[推断]` 当前不应把 #44 的“等待上游”触发器当作已完成条件；但现在存在一条不需要 `allow-dirty` 的原生注入点，值得先做短周期原型验证，再决定是否合并。

建议按以下顺序决策：

1. 先用 `pr-run-mode = "skip"` 消除 Release 工作流的 PR 触发器；确认 `release-drift` 仍能在 PR 上覆盖生成文件与 action pin 的对账。
2. 在隔离分支实现方案 G：增加 bootstrap wrapper、版本/摘要清单与一致性检查；用 `dist generate --check`、`dist plan` 和本地 mock 校验 shell 路径，Windows/macOS 走一次矩阵 dry-run。
3. 若方案 G 的维护成本可接受，则先合并第一阶段并保持 #44 open；第二阶段完成 override pin 后再关闭。若不可接受，再退回“等待上游”，或在明确记录风险后采用 `allow-dirty = ["ci"]` 的受控手改方案。
4. 任何情况下都记录撤销条件：当 #2387（root read + 内置 job write）或 #2420（installer checksum）进入正式版本且覆盖当前模板时，重新评估是否移除 override/wrapper。

不建议：

- 不建议引入 `pull_request_target` 来复用 Release 工作流的写 token 或 secrets。
- 不建议只做“手改 + `allow-dirty`”而不增加重新生成/补丁对比；否则升级时容易静默偏离模板。
- 不建议把“上游 installer 的 SHA-256”当作唯一信任根；它防的是 pin 之后的替换，不替代对固定版本、wrapper 来源 ref 与升级 diff 的审查。

## 7. 独立测试记录（2026-10-19）

| 测试 | 结果 | 证据类型 |
| --- | --- | --- |
| 计算 `cargo-dist-installer.sh` SHA-256 | `c5b53ac2559eb26e3b3dc28755eb5d011ed6ec9ac97fa4fc2a2459872e766b5e`，与 GitHub API digest 一致 | 独立测试 |
| 校验 `sha256.sum` 是否包含 installer | 不包含 | 独立测试 |
| `gh attestation verify` 安装器脚本 | 404，无 attestation | 独立测试 |
| 计算 Linux 归档 SHA-256 | `4b3f0a5f0ebbdb798f6db649d01b32ba1518376b6f7a0502b7d92b75cc2c8293`，与 API/`sha256.sum` 一致 | 独立测试 |
| `gh attestation verify` Linux 归档 | 通过 | 独立测试 |
| 追加 `--signer-workflow` 与 `--source-ref refs/tags/v0.33.0` | 通过 | 独立测试 |
| `cargo-dist-url-override` + `pr-run-mode = "skip"` 生成实验 | `dist generate` 成功；shell/PowerShell 命令均改用 override；无 `pull_request`；`dist generate --check` 退出 0 | 独立测试 |

## 8. 仍未验证或存在不确定性的点

`[待验证]`

1. `[待验证]` 同仓库 PR 上 `plan` job 的实际 `GITHUB_TOKEN` 是否一定为 write：官方规则只对 fork PR 做强制降级；本调研未在真实同仓库 PR 上通过 API 检查 `X-OAuth-Scopes` 或实际写操作验证。
2. `[待验证]` 在 GitHub Actions job 中只带 `contents: read` 且不显式传 `GH_TOKEN` 时，`gh attestation verify` 对 public repository 是否总能匿名/低权限访问 attestation API；本调研只在本地 CLI 环境验证成功。
3. `[待验证]` #2387 是否会被拆分或最终合并；截至 2026-10-19 它仍 open、冲突，且维护者对其安全定性有保留。
4. `[待验证]` #2420 是否会在 `v0.34` 或后续版本实现；当前无评论、无关联 merged PR。
5. `[待验证]` cargo-dist `0.33.0` 是否会补发到 crates.io；API 当日返回 403，只能依据 #2506 的维护者说明。
6. `[待验证]` 如果采用 `allow-dirty` + 补丁重放，`dist generate` 在临时目录中是否能在不触碰仓库其他文件的前提下稳定完成；这需要在实现 #44 时设计一次性验证脚本。
7. `[待验证]` 上游是否会在未来把 installer 也纳入 GitHub attestation；当前独立测试显示没有。
8. `[待验证]` 方案 G 的 wrapper 在 macOS ARM64 与 Windows x86_64 runner 上的 shell/PowerShell 语法、临时文件清理、错误传播与 SHA-256 工具可用性尚未实跑；同时尚未确定 commit pin 的更新流程（引入时可能需要“先合并 wrapper、再把 override 指向该 merge commit”的两步流程）。

## 9. 实施进展（2026-10-20）

`[独立测试]` 已完成方案 G 的第一阶段实现，尚未启用 `cargo-dist-url-override`：

- 新增 `.github/dist-bootstrap/cargo-dist-installer.sh`、`.github/dist-bootstrap/cargo-dist-installer.ps1` 与 `.github/dist-bootstrap/cargo-dist-installer.sha256`。
- shell wrapper 的离线自测、真实下载与 SHA-256 校验均通过；通过 `curl ... | sh` 方式执行也通过。
- 使用临时 `CARGO_HOME` 执行 shell wrapper 后，安装出的 `dist --version` 为 `cargo-dist 0.33.0`。
- PowerShell wrapper 在本地 PowerShell 容器中的离线自测与语法解析通过；真实 `-Check` 下载受本地网络限制，仍待 Windows CI 确认。
- CI 的 `release-drift` 已改用仓库内 shell wrapper；macOS/Windows job 分别运行各自平台的 `--check`，真实矩阵结果仍待推送分支后确认。

`[待验证]` 第二阶段必须先把 wrapper 合并到 `main`，再在后续 PR 把 `cargo-dist-url-override` 指向该 main commit，并运行 `dist generate`。当前分支不能在同一 squash PR 中自引用 pinned commit。

## 10. 关键来源清单

### issue / PR / release

- #44：<https://github.com/geeknonerd/stuntdouble/issues/44>（2026-10-19 核验）
- cargo-dist v0.33.0：<https://github.com/axodotdev/cargo-dist/releases/tag/v0.33.0>（2026-10-19 核验）
- cargo-dist #2506：<https://github.com/axodotdev/cargo-dist/issues/2506>（2026-10-19 核验）
- cargo-dist #2420：<https://github.com/axodotdev/cargo-dist/issues/2420>（2026-10-19 核验）
- cargo-dist #2352：<https://github.com/axodotdev/cargo-dist/pull/2352>（2026-10-19 核验）
- cargo-dist #2387：<https://github.com/axodotdev/cargo-dist/pull/2387>（2026-10-19 核验）
- cargo-dist #2279：<https://github.com/axodotdev/cargo-dist/issues/2279>（2026-10-19 核验）
- cargo-dist #2133/#2134：<https://github.com/axodotdev/cargo-dist/issues/2133>、<https://github.com/axodotdev/cargo-dist/pull/2134>（2026-10-19 核验）
- cargo-dist #1497：<https://github.com/axodotdev/cargo-dist/pull/1497>（2026-10-19 核验）
- cargo-dist #2497：<https://github.com/axodotdev/cargo-dist/pull/2497>（2026-10-19 核验）
- cargo-dist #909/#1179：<https://github.com/axodotdev/cargo-dist/issues/909>、<https://github.com/axodotdev/cargo-dist/pull/1179>（2026-10-19 核验）
- cargo-dist #1509（`cargo-dist-url-override`）：<https://github.com/axodotdev/cargo-dist/pull/1509>（2026-10-19 核验）

### cargo-dist 源码/文档

- 权限硬编码：<https://github.com/axodotdev/cargo-dist/blob/v0.33.0/cargo-dist/src/backend/ci/github.rs#L306-L310>
- Release 模板权限输出：<https://github.com/axodotdev/cargo-dist/blob/v0.33.0/cargo-dist/templates/ci/github/release.yml.j2#L26-L31>
- PR 触发器模板：<https://github.com/axodotdev/cargo-dist/blob/v0.33.0/cargo-dist/templates/ci/github/release.yml.j2#L65-L68>
- installer 自安装命令：<https://github.com/axodotdev/cargo-dist/blob/v0.33.0/cargo-dist/src/backend/ci/mod.rs#L123-L146>
- installer 归档校验：<https://github.com/axodotdev/cargo-dist/blob/v0.33.0/cargo-dist/templates/installer/installer.sh.j2#L233-L305>、<https://github.com/axodotdev/cargo-dist/blob/v0.33.0/cargo-dist/templates/installer/installer.sh.j2#L1617-L1655>
- `cargo-dist-url-override` 分支：<https://github.com/axodotdev/cargo-dist/blob/v0.33.0/cargo-dist/src/backend/ci/mod.rs#L88-L146>
- checksum 文档：<https://github.com/axodotdev/cargo-dist/blob/v0.33.0/book/src/artifacts/checksums.md#L1-L19>
- `allow-dirty`：<https://github.com/axodotdev/cargo-dist/blob/v0.33.0/book/src/reference/config.md#L141-L161>
- 手改 release.yml：<https://github.com/axodotdev/cargo-dist/blob/v0.33.0/book/src/ci/customizing.md#L340-L362>
- `pr-run-mode`：<https://github.com/axodotdev/cargo-dist/blob/v0.33.0/book/src/reference/config.md#L1420-L1440>
- `github-custom-job-permissions`：<https://github.com/axodotdev/cargo-dist/blob/v0.33.0/book/src/reference/config.md#L1498-L1515>

### GitHub 官方

- 权限计算与 job 继承：<https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax#permissions>
- fork write 降级：<https://github.com/github/docs/blob/main/data/reusables/actions/forked-write-permission.md>
- job 级权限未列项设为 none：<https://github.com/github/docs/blob/main/data/reusables/actions/github-token-available-permissions.md>
- 安全使用 workflows：<https://github.com/github/docs/blob/main/content/actions/reference/security/secure-use.md>
- `pull_request_target` 安全指南：<https://github.com/github/docs/blob/main/content/actions/reference/security/securely-using-pull_request_target.md>
- artifact attestations：<https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations/use-artifact-attestations>
- artifact attestations 概念与 SLSA 等级：<https://github.com/github/docs/blob/main/content/actions/concepts/security/artifact-attestations.md>

### 供应链标准

- SLSA v1.2：<https://slsa.dev/spec/v1.2/>
- SLSA v1.2 验证 artifact：<https://slsa.dev/spec/v1.2/verifying-artifacts>
- SLSA v1.2 builder 要求：<https://slsa.dev/spec/v1.2/build-requirements>
- OpenSSF Scorecard checks：<https://github.com/ossf/scorecard/blob/main/docs/checks.md>
- NIST SP 800-218：<https://nvlpubs.nist.gov/nistpubs/SpecialPublications/NIST.SP.800-218.pdf>
- Sigstore cosign 验证：<https://docs.sigstore.dev/cosign/verifying/verify/>

# Issue #44 后续评估：cargo-dist Bootstrap 的摘要绑定与分阶段落地

- 日期：2026-10-20（Asia/Shanghai）
- 状态更新：本文记录 review 当时的问题与建议；当前实现已按建议改为 wrapper 内嵌版本与摘要、manifest 仅作审计，并以 [ADR 0018](../plans/adr/0018-cargo-dist-bootstrap-verification.md) 为准。
- 关联决策：[ADR 0018](../plans/adr/0018-cargo-dist-bootstrap-verification.md)
- 范围：评估 #44 第一阶段未提交改动；重点回答 `cargo-dist-url-override` 的信任边界、上游状态、跨平台校验一致性与分阶段合并条件。
- 证据标记：`[官方声明]`、`[源码事实]`、`[独立测试]`、`[推断]`、`[待验证]`。

## 执行摘要

1. `[官方声明 + 推断]` cargo-dist 最新正式版仍为 `v0.33.0`（2026-09-11）；#2387、#2420、#2497 均未合并，暂无原生替代。
2. `[官方声明 + 源码事实]` `cargo-dist-url-override` 是 #1509 的“quick iteration”配置，覆盖 `cargo-dist-version`，仍生成 `curl … | sh` / `irm … | iex`，不是安全 API。
3. `[源码事实 + 推断]` wrapper 从当前 checkout 读取版本和 `.sha256`，而 dispatch 在 tag ref 上运行，因此 pin 只固定代码、不固定摘要；code review 的争议成立。
4. `[源码事实]` shell 与 PowerShell 对同一清单的解析规则不一致，且自测 fixture 使用不同内容和不同 hash，无法证明两条路径语义等价。
5. `[独立测试 + 推断]` 2026-10-20 两个 installer 摘要与当前清单一致；这只证明数值正确，移动 tag checkout 中的清单仍可改变预期值。
6. `[推断]` 建议允许合并第一阶段作为明确标记的 prerequisite，但 #44 保持 open；第二阶段在 pinned wrapper 中内嵌版本与两个平台摘要，再把 override 指向已进入 `main` 的 full commit。embedded digest 是当前最小且最易审计的方案。

## 1. 上游定义、用途与风险

`[官方声明]` #1509（2024-11-06 合并）称该键用于“for quick iteration on cargo-dist itself”。源码注释将其定义为安装 dist 的 URL：覆盖 `cargo_dist_version`，并要求类似自建 `./target/distrib` 的目录结构。

`[源码事实]` `install_strategy()` 先检查 URL override，再按版本计算官方 release URL；override 分支固定使用 `cargo-dist-installer` 名称，因此生成 `<URL>/cargo-dist-installer.sh` 或 `.ps1`。它只改变下载前缀，不改变“未校验执行脚本”的默认语义。

`[推断]` 风险包括：branch/tag/short SHA 可移动；URL 可指向任意主机或仓库；运行时输入若来自当前 checkout，pin 不覆盖摘要或版本白名单。第三类正是 review 当时实现的具体缺陷。

| 上游项 | 2026-10-20 状态 | 与本议题的关系 |
| --- | --- | --- |
| `v0.33.0` | 最新正式版，target commit `a1c829ff…` | 仓库当前仍使用该版本 |
| #2387 | open、`CONFLICTING`；包含 root `contents: read` 与 host/announce job write，但未合并 | 可解决权限部分，不能依赖 |
| #2420 | open、无评论；请求 checksum-pinned 自安装或 `cargo install` 变体 | 未提供原生摘要绑定 |
| #2497 | open；为 PowerShell 安装器补归档 checksum，shell 早先已有 | 直接证明两平台曾存在校验漂移 |

`[源码事实]` `v0.33.0` 与 `main` HEAD `d1055f48…`（2026-10-04）仍硬编码顶层 `contents: write`（内置 job 没有 per-job 权限），生成的安装命令仍是 `curl | sh`（没有 checksum-pinned installer）；#2387/#2420 未合并，因此 #44 的两个上游触发条件都未满足，“等待上游”不是短期完成路径。

`[待验证]` 截至核验日未检索到已合并的后续替代实现；这不等于上游永远不会实现，需继续跟踪 release notes 与上述条目。

## 2. pin 的信任边界

`[源码事实]` shell wrapper 的 `repo_root()` 在 `curl | sh` 场景返回 `pwd`，再从该根目录拼接 `dist-workspace.toml` 和 `.sha256`（`.github/dist-bootstrap/cargo-dist-installer.sh:16-24,183-187`）。PowerShell 在 `$PSScriptRoot` 为空时返回 `Get-Location`（`.github/dist-bootstrap/cargo-dist-installer.ps1:7-14,135-139`）。

`[官方声明]` `workflow_dispatch` 的 `GITHUB_REF`/`GITHUB_SHA` 对应该次 dispatch 的 branch 或 tag；`release-plz` 又以 `gh workflow run release.yml --ref "$tag"` 触发（`.github/workflows/release-plz.yml:44`）。因此 release 运行看到的是 tag checkout，而 pinned override 只固定了被下载的 wrapper 文件。

`[源码事实]` review 当时的仓库 `release.yml` 仍是顶层 `contents: write`、含 `pull_request` 触发器、在 plan job 直接 `curl … | sh`（`.github/workflows/release.yml:17-18,41-43,71`）；当时的第一阶段代码只改善了 CI/release-drift 的安装入口，尚未完成权限与 Release installer 验收。

`[推断]` 攻击者可改 tag checkout 中的清单，进而选择不同官方版本或制造校验失败。若无法替换上游资产，影响可能是降级或拒绝服务；若 tag/manifest 与上游资产同时被控制，则可能执行非预期版本。review 时的设计仍是“pinned code + mutable expected value”，没有形成“code + checked digest”的绑定。

`[独立测试]` 本日重新下载上游 `cargo-dist-installer.sh` 与 `.ps1`，SHA-256 与仓库清单一致。该结果只用于确认当前清单没有算错，不能消除上述信任边界问题。

## 3. 同类实践：摘要应放在哪里

`[源码事实]` `ewhauser/bazel-mcp` 在 commit `fbb07b819…` 的 shell/PowerShell 安装脚本中直接内嵌 `DIST_VERSION` 与 installer SHA-256；脚本下载后分别用 `shasum`/`Get-FileHash` 校验，失败即停止，再执行安装器。这就是“pinned wrapper 同时固定代码与摘要”的真实源码样例。

`[源码事实]` cargo-dist 自己生成的 shell 安装器也把归档 digest 嵌入生成脚本，再校验下载的归档；但对应 PowerShell 安装器长期缺少同类校验，直到 #2397/#2497 才提出补齐。这说明 embedded digest 是上游已经采用的模式，但两个平台并不自动保持一致。

`[官方声明]` SLSA v1.2 验证步骤要求 provenance 的 `subject` 与 artifact digest 匹配，并验证 builder 身份、签名与预期参数；NIST SSDF `PS.2.1` 要求向获取方提供完整性验证信息，`PS.3.1/PS.3.2` 要求安全归档并保护 provenance。它们支持“让消费方可验证”，但把摘要内嵌到 pinned wrapper 是本项目的实现推断，不是标准规定的唯一形式。

`[推断]` 优先级为 embedded digest > pinned manifest URL > archive + attestation：前者把版本与双平台摘要直接固定进 wrapper；中者固定同一 commit 的清单 URL，但多一次网络读取与解析；后者使用 `gh attestation verify --repo … --signer-workflow … --source-ref …`，provenance 更强，但 attestation 不保证内容安全，且当前 installer 脚本本身没有可验证 attestation。

## 4. 适用标准与检查器的边界

`[官方声明 + 推断]` GitHub Actions 安全指南当前把 workflow dependency lockfile 列为最佳措施；没有 lockfile 时，第三方 action/workflow 应 pin 到 full-length commit SHA，因为 tag 可移动。该指南直接覆盖 action 与 reusable workflow，不自动保证 shell 运行时下载物的摘要一致；把同一原则扩展到 bootstrap wrapper 属于合理推断。

`[官方声明]` GitHub 还提供 immutable release（锁定 tag 与资产并生成 release attestation）和 artifact attestations；`gh attestation verify` 可约束仓库、签名 workflow 与 source ref。Fork PR 的 `GITHUB_TOKEN` 会被降为只读，但不能替代 job 级最小权限与自举校验。

`[官方声明 + 推断]` OpenSSF Scorecard 的 `Pinned-Dependencies` 明确检查 Dockerfile、shell 脚本与 GitHub workflow 是否按 hash 固定依赖；`Token-Permissions` 要求顶层只读、写权限放到单个 job。它是独立自动化评估工具，因此不是 GitHub 的强制合规门槛（推断）。

`[官方声明 + 推断]` NIST 当前正式版仍是 SP 800-218 v1.1（2022-02），Rev.1/SSDF v1.2 仍是 2025-12 初始公开草案。相关任务是 `PO.3.2`（安全维护工具链、可复现构建）、`PS.1.1`（代码完整性）、`PS.2.1`（提供完整性校验信息）、`PS.3.1/PS.3.2`（归档 release 与 provenance）。这些是结果导向任务，本文的 SHA-256 具体实现属于映射推断。

## 5. review 时发现的跨平台漂移与验证要求

`[源码事实]` review 当时的差异包括：

- shell 只检查目标版本的字段/行，其他版本的畸形行会被忽略；PowerShell 对每个非空非注释行都要求恰好三个字段。
- shell 的 `read` 在 CRLF 清单上会把 `\r` 留进匹配行而报 malformed；PowerShell 用 `.Trim()` 后通常可接受 CRLF。
- shell 的十六进制校验只接受小写字母；PowerShell 同样只匹配小写，但比较 hash 时会把两侧转成小写。
- shell 自测 fixture 含换行且使用一个已知 hash；PowerShell fixture 不含换行且使用另一个 hash，负例与边界覆盖也不同。
- shell 将 installer 作为子进程执行，PowerShell 通过 `Invoke-Expression` 在当前会话执行。

`[官方声明]` #2497 的测试计划声明在 Windows PowerShell 5.1 与 7.6.5 上测试有效归档、无效摘要与 fallback；这证明上游也把跨版本、真实归档的矩阵测试作为必要证据。

`[推断]` 降低漂移的有效组合是：同一严格 schema 的单一清单；共享正反测试向量（重复版本、未知版本、额外字段、CRLF、大小写、空格与注释）；在 ubuntu/macOS/Windows 矩阵中运行同一组 `--self-test` 与 `--check`；并让两条实现共享“比较摘要”的语义。仅“生成一份清单”不够，因为解析器仍可各自解释。

## 6. 分阶段修复建议

`[推断]` 可以合并第一阶段作为 prerequisite，但必须同时满足：

1. issue #44 保持 open，三条验收标准不得勾选；PR/ADR 明确写“Phase 1 prerequisite”，并链接第二阶段的独立 PR 或跟踪项。
2. 文档不得声称“摘要由 pin 固定”；在 embedded digest 落地前，只能描述为“wrapper 代码由 pin 固定、摘要仍待 Phase 2 绑定”。
3. CI 先加入 fail-closed 检查：wrapper 自身的版本/摘要校验必须在矩阵执行；override 尚未启用时，显式断言 `release.yml` 仍由生成器管理。Phase 2 还必须满足所有安装入口经过 wrapper、release workflow 顶层 `contents: read`、无 `pull_request`，且 override 是 40 位 full commit SHA 的受控 raw URL。
4. `--self-test`/`--check` 在 Linux、macOS、Windows 全矩阵执行；`dist generate --check` 继续作为漂移门禁。
5. 第一阶段合并后记录 `main` commit，第二阶段单独更新 override 并重新生成 `release.yml`。在第二阶段通过前，#44 不能标记完成；若长期不推进，应回滚未绑定摘要的部分或把它继续视为未修复风险。

## 7. 方案比较与推荐

| 方案 | code+digest 绑定 | 维护与信任边界 | 结论 |
| --- | --- | --- | --- |
| review 时“pinned wrapper + 运行时读清单” | 否 | 代码被 pin，清单随 tag checkout 变化 | 拒绝作为最终态 |
| wrapper 内嵌版本与双平台摘要 | 是 | 每次升级需改 wrapper 并走两阶段 pin | **推荐** |
| wrapper 硬编码同 commit 的清单 URL | 是 | 多一层 fetch/解析，但可接受 | 次选 |
| archive + attestation | 是（provenance/digest） | 依赖 `gh` 与 attestation 服务，改动更大 | 上游提供 attested installer 时的 fallback |
| 等待上游 #2387/#2420 | 否 | 现状风险持续存在 | 只能并行跟踪 |

`[推断]` 最小改动路径：先在两个 wrapper 中内嵌 `0.33.0` 与当前两个 SHA-256，读取 `dist-workspace.toml` 只用于断言版本一致，然后增加共享测试向量和矩阵门禁；合并后在第二阶段把 `cargo-dist-url-override` 指向该 merge commit，再生成 `release.yml`。这样 pin 同时固定代码与预期摘要，摘要升级必须形成可审查 diff，也避免额外维护一个运行时可变清单。

## 来源清单

以下链接均于 2026-10-20 核验。关键内容已通过官方页面或其官方 raw/API 源复核；本次网络环境中个别 HTTPS 请求出现瞬时 TLS 中断，但未影响结论。

- cargo-dist #1509：<https://github.com/axodotdev/cargo-dist/pull/1509>
- cargo-dist `cargo-dist-url-override` 定义：<https://github.com/axodotdev/cargo-dist/blob/v0.33.0/cargo-dist/src/config/v0.rs#L22-L28>
- cargo-dist installer 策略与 override 优先级：<https://github.com/axodotdev/cargo-dist/blob/v0.33.0/cargo-dist/src/backend/ci/mod.rs#L88-L146>
- cargo-dist 顶层权限硬编码：<https://github.com/axodotdev/cargo-dist/blob/v0.33.0/cargo-dist/src/backend/ci/github.rs#L306-L310>
- cargo-dist 最新 release：<https://github.com/axodotdev/cargo-dist/releases/tag/v0.33.0>；API 核验：<https://api.github.com/repos/axodotdev/cargo-dist/releases/latest>
- cargo-dist `main` HEAD：<https://github.com/axodotdev/cargo-dist/commit/d1055f48071e3b253896e85ed0bcb8064af50512>
- cargo-dist #2387：<https://github.com/axodotdev/cargo-dist/pull/2387>
- cargo-dist #2420：<https://github.com/axodotdev/cargo-dist/issues/2420>
- cargo-dist #2397：<https://github.com/axodotdev/cargo-dist/issues/2397>
- cargo-dist #2497：<https://github.com/axodotdev/cargo-dist/pull/2497>
- `ewhauser/bazel-mcp` embedded digest 样例：<https://github.com/ewhauser/bazel-mcp/blob/fbb07b8191530b6a8fc1aef1e1a4d3cf5971c582/scripts/install-cargo-dist.sh>、<https://github.com/ewhauser/bazel-mcp/blob/fbb07b8191530b6a8fc1aef1e1a4d3cf5971c582/scripts/install-cargo-dist.ps1>
- 项目既有独立测试记录（v0.33.0 Linux 归档 attestation 通过、installer attestation 404）：`research/44-dist-ci-hardening.md`
- GitHub Actions 安全使用与 SHA pin：<https://docs.github.com/en/actions/reference/security/secure-use>
- `workflow_dispatch` ref 语义：<https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#workflow_dispatch>
- GitHub immutable release：<https://docs.github.com/en/code-security/supply-chain-security/understanding-your-software-supply-chain/immutable-releases>
- GitHub artifact attestations 与 `gh attestation verify`：<https://docs.github.com/en/actions/concepts/security/artifact-attestations>、<https://cli.github.com/manual/gh_attestation_verify>
- SLSA v1.2 artifact verification：<https://slsa.dev/spec/v1.2/verifying-artifacts>
- OpenSSF Scorecard checks：<https://github.com/ossf/scorecard/blob/main/docs/checks.md#pinned-dependencies>、<https://github.com/ossf/scorecard/blob/main/docs/checks.md#token-permissions>
- NIST SP 800-218 v1.1：<https://csrc.nist.gov/pubs/sp/800/218/final>；PDF：<https://nvlpubs.nist.gov/nistpubs/specialpublications/nist.sp.800-218.pdf>
- NIST SP 800-218 Rev.1（SSDF v1.2 草案）：<https://csrc.nist.gov/pubs/sp/800/218/r1/ipd>

仓库内事实位置：`.github/dist-bootstrap/cargo-dist-installer.sh:16-24,59-98,183-198`、`.github/dist-bootstrap/cargo-dist-installer.ps1:7-14,33-80,135-156`、`.github/workflows/release-plz.yml:44`、`.github/workflows/release.yml:17-18,41-43,71`、`.github/workflows/ci.yml:54-88,90-101`。

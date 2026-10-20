# cargo-dist 自举安装：不可变 wrapper 与摘要校验

- 状态：`已接受（分两阶段落地）`
- 日期：2026-10-20
- 作者：@geeknonerd
- 关联：[#44](https://github.com/geeknonerd/stuntdouble/issues/44)、[ADR 0012](0012-release-artifacts-and-supply-chain.md)、[开发指南](../../docs/development.md)、[调研报告](../../research/44-dist-ci-hardening.md)、[后续评估](../../research/44-bootstrap-followup-evaluation.md)

## 背景

cargo-dist `0.33.0` 生成的工作流仍有三个问题：

1. 工作流顶层硬编码 `contents: write`，并保留 `pull_request` 触发器，使 Release 的 plan job 在 PR 上继承发布写权限。
2. plan job 与构建 matrix 通过 `curl ... | sh` 或 `irm ... | iex` 直接执行上游 installer，没有校验 installer 自身的摘要。
3. 上游尚未提供内置 job 的 per-job permissions，也没有 checksum-pinned installer；等待上游会让 #44 继续开放。

`allow-dirty = ["ci"]` 可以支持手改生成文件，但会让 `dist generate --check` 不再覆盖 `release.yml`，每次升级 cargo-dist 都要重放补丁。项目不退化为手工维护生成文件。

## 决策

1. **PR 不运行 Release workflow。** 设置 `pr-run-mode = "skip"`，由常规只读 CI 承担 PR 验证。生成的 `release.yml` 只保留 `workflow_dispatch`，因此 fork 与同仓库 PR 都不会获得 Release workflow 的写权限。
2. **增加自包含 bootstrap wrapper。** `.github/dist-bootstrap/` 保存 shell/PowerShell wrapper 与版本-摘要清单；两个 wrapper 各自内嵌目标 `cargo-dist` 版本和对应 installer 的 SHA-256，只从 `dist-workspace.toml` 读取版本用于断言一致，下载并校验后才执行。`.sha256` 清单是审计来源，由 CI 检查脚本统一解析，wrapper 不在运行时读取它。
3. **用不可变 commit pin 注入 wrapper。** `cargo-dist-url-override` 必须指向 `https://raw.githubusercontent.com/geeknonerd/stuntdouble/<40 位 full commit SHA>/.github/dist-bootstrap`。该 commit 同时固定 wrapper 代码、版本断言和两个平台的内嵌摘要，形成 code + digest 绑定。
4. **不启用 `allow-dirty`。** `release.yml` 继续由 `dist generate` 生成，`release-drift` 保留 `dist generate --check`，并增加 override pin、wrapper 内容与摘要清单的一致性检查。
5. **分两阶段合并。** 第一阶段合并 wrapper、摘要、自检、CI 接入和 `pr-run-mode = "skip"`，但 Release workflow 仍使用上游默认 installer；第二阶段把 override 指向第一阶段产生的、已经在 `main` 上的 full commit SHA，并重新生成 `release.yml`。禁止在同一个 squash PR 内把 override 指向该 PR 尚不可达的 commit。

## 后果

**正面**

- Release workflow 不再有 PR 触发器，PR 无法获得发布写权限。
- 所有经 bootstrap wrapper 的安装（当前 CI/release-drift，第二阶段起的 Release）在执行前必须匹配其内嵌 SHA-256；替换 installer、版本或摘要会形成可审查的 diff。
- 生成文件仍由 cargo-dist 管理，升级时不需要重放手写补丁。
- Linux、macOS 与 Windows 分别覆盖 `sha256sum`/`shasum` 与 `Get-FileHash` 路径。

**负面（接受）**

- 需要维护 wrapper 和每个 cargo-dist 版本的两种 installer 摘要。
- `cargo-dist-url-override` 是上游的快速迭代配置，不是正式安全 API；wrapper 的完整性与内嵌摘要依赖 full commit pin，运行期只负责按该摘要校验下载物。
- PR 不再自动运行为 Release 准备的计划检查；常规 CI 只验证生成漂移、bootstrap 和项目门禁。
- 第一次启用与每次更新 pin 都需要两阶段发布流程，不能在一个 squash PR 内自引用。

## 撤销条件

当 cargo-dist 正式版本同时提供以下能力时，重新评估并优先移除 override/wrapper：

- 内置 Release job 的最小权限配置，PR 不再继承发布写权限；
- 带固定摘要或 attestation 的 installer 获取方式；
- 上述能力可通过 `dist generate` 使用，且不需要 `allow-dirty`。

移除时恢复上游默认 installer URL，重新生成 `release.yml`，并同步删除 bootstrap 目录、CI 检查和本 ADR 指向的维护步骤。

## 替代方案

- **继续等待上游**：拒绝作为唯一方案；#44 的暴露面仍然存在。
- **`allow-dirty = ["ci"]` + 手改**：拒绝；生成文件漂移检查失效，升级维护成本高于 wrapper。
- **`pull_request_target`**：拒绝；会把不可信 PR 与高权限上下文重新组合。
- **直接运行归档下载并验证 attestation**：保留为上游 installer 无法摘要校验时的 fallback；当前方案可在不手改生成工作流的前提下完成 installer 校验。

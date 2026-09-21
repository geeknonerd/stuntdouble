# 配置、`ctx` API 与 CLI 各自独立兼容契约

- 状态：`已接受`
- 日期：2026-09-19
- 关联：[ADR 0003](0003-script-first-multi-runtime.md)、[ADR 0004](0004-host-functions-only-sandbox.md)、[ADR 0010](0010-git-and-release-workflow.md)

## 背景

Stunt Double 暴露三个公开面：配置文件、脚本宿主 API（`ctx`）与 CLI。它们以不同速度演进。单一产品版本号无法安全描述这三者，而且移除宿主函数会破坏用户脚本，即使服务端本身仍然兼容。

## 决策

把三个面当作独立契约。

### 配置

- 主格式是 TOML。
- 默认文件为 `stuntdouble.toml`；`--config <path>` 覆盖它。
- `0.x` 版本可以在给出弃用窗口后变更 schema。
- `1.0` 及以后按 SemVer 保证向后兼容。
- schema 细节见 `docs/contracts/config.md`。

### `ctx` 宿主 API

- 每个脚本都能看到 `ctx.apiVersion`。
- Version 1 是第一个契约。
- 同一个 `apiVersion` 内可以新增宿主函数，但不得移除或改名。
- 移除或改名函数需要新的 `apiVersion`。
- 产品可以同时支持多个 `apiVersion`。
- 对应的 `.d.ts` 文件是契约的一部分。
- 细节见 `docs/contracts/ctx-api.md`。

### CLI

- 命令、flag 与退出码是公开契约。
- 退出码：`0` 成功，`1` 运行时错误，`2` 配置错误，`3` 内部错误。
- `0.x` 版本可以在给出弃用窗口后变更 CLI。
- `1.0` 及以后遵循 SemVer。
- 细节见 `docs/contracts/cli.md`。

## 弃用规则

移除或改名任何公开面之前，至少提前一个 minor 版本给出警告。警告同时写入日志与 `CHANGELOG.md`。破坏性变更要在 release notes 中给出迁移示例。

## 后果

- 契约文档必须与实现同步更新，而不是事后补。
- `.d.ts` 发布步骤是发布清单的一部分。
- `1.0.0` 要求配置、CLI 与 `ctx` API version 1 稳定。

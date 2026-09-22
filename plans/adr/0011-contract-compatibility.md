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

## 修订（T10，#33）：信号驱动的关闭退出码

原 CLI 决策只列出 `0–3` 退出码。T10 增加服务生命周期退出码，原有四类错误码语义不变：

- 首次 SIGINT（Ctrl-C）或 SIGTERM 触发 graceful shutdown：停止接受新连接，排空在途请求，完成后退出 `0`。
- 首次信号已经被观测后，再次收到 SIGINT/SIGTERM：放弃排空并立即终止，退出码沿用 shell 的 128+signal 约定；SIGINT/Ctrl-C 为 `130`，SIGTERM 为 `143`。
- 标准信号不排队。两个信号在第一个被观测前背靠背到达时可能合并为一个通知；该退化路径只执行第一次信号的 graceful shutdown，正常排空并退出 `0`，不承诺强制退出。
- Windows 只提供 Ctrl-C，语义等同 SIGINT。当前 CI 只运行 Linux；Windows 分支目前只覆盖目标平台类型检查，行为自动化待引入 Windows runner 后补充。
- 本修订发生在首个 tag 之前（远程无已发布 tag），因此不适用“提前一个 minor 版本警告”的弃用窗口；迁移说明记录在 `CHANGELOG.md`。

## 弃用规则

移除或改名任何公开面之前，至少提前一个 minor 版本给出警告。警告同时写入日志与 `CHANGELOG.md`。破坏性变更要在 release notes 中给出迁移示例。

## 后果

- 契约文档必须与实现同步更新，而不是事后补。
- `.d.ts` 发布步骤是发布清单的一部分。
- `1.0.0` 要求配置、CLI 与 `ctx` API version 1 稳定。

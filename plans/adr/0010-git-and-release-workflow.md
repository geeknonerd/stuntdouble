# 主干开发与发布流程

- 状态：`已接受`
- 日期：2026-09-19
- 关联：[ADR 0008](0008-dual-mit-apache-license.md)、[开发指南](../../docs/development.md)

## 背景

仓库在实现开始前需要可预测的工作流。当前仓库只有一个 `main` 分支，没有 tag、没有 release，也没有分支保护。发布需要在三个平台上自动化且可复现。

候选方案是主干开发、带 merge commit 的 GitHub Flow，以及 GitFlow。

## 决策

采用主干开发 + 仅允许 squash merge。

- `main` 始终可发布。
- 所有工作走短生命周期分支与 pull request。
- 只允许 squash merge；合并后删除分支。
- `main` 受保护，禁止直接 push、force push 与删除。
- CI 是硬门槛。只有一位维护者时所需批准数为 0；第二位维护者加入后变为 1。
- 发布从 `main` 打 tag，不使用 `develop` 分支。
- 1.0 之后，旧版本线使用从 tag 创建的 `release/x.y` 分支。
- release PR 由 `release-plz` 生成；合并 release PR 即授权打 tag 与发布。
- 发布失败绝不复用或覆盖 tag，而是发布新的 patch 或预发布版本。

## 替代方案

- **带 merge commit 的 GitHub Flow**：更简单，但历史噪声大，CHANGELOG 自动化更弱。
- **GitFlow**：适合并行维护多个版本，对当前项目过重。
- **不定义工作流**：与自动发布和分支保护不兼容。

## 后果

- pull request 标题必须遵循 Conventional Commits，因为 squash commit 会继承它。
- DCO 签名是强制的。
- 分支保护与必需状态检查成为发布契约的一部分。
- 发布自动化必须从 tag 运行，绝不直接从 `main` 运行。

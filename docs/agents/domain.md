# 领域文档

工程技能在探索代码库时应如何消费本仓库的领域文档。

## 探索之前先读这些

- 仓库根目录的 **`CONTEXT.md`** —— 项目词汇表。
- **`plans/adr/`** —— 读与你要改动区域相关的 ADR。本仓库把 ADR 放在这里，而不是 `docs/adr/`。
- **`plans/product-definition.md`** —— v1 范围、宿主 API 契约与不做清单。
- **`plans/demo-document-catalog.md`** —— 生成 CSV 清单与二进制下载的公开演示场景；对应的可运行夹具在 `demo/`。
- **`docs/contracts/`** —— 配置、`ctx` API 与 CLI 契约。
- **`types/ctx-api-v1.d.ts`** —— 已实现的 `ctx` apiVersion 1 子集的源码类型定义。
- **`docs/guide/`** —— 面向使用者的快速开始与任务说明（B 层，英文 + `.zh-CN.md` 译本）。
- **`docs/development.md`** —— Git 工作流、CI、版本号与发布规则。

文件不存在就**静默跳过**，不要指出缺失。领域术语收敛后更新 `CONTEXT.md`。

## 布局

本仓库是**单上下文**：

```text
/
├── AGENTS.md
├── README.md
├── README.zh-CN.md
├── CONTRIBUTING.md
├── GOVERNANCE.md
├── SECURITY.md
├── CHANGELOG.md
├── CONTEXT.md          ← 项目词汇表
├── demo/               ← 文档清单演示场景的可运行夹具
├── types/              ← apiVersion 1 的 `ctx` 类型定义
├── src/                ← config、matcher、script、server、upstream（内部）模块
├── tests/              ← 通过构建出的二进制做端到端检查
├── docs/
│   ├── README.md       ← 维护者文档索引（中文）
│   ├── index.md        ← GitHub Pages 首页（配 index.zh-CN.md）
│   ├── guide/          ← 使用指南（英文 + .zh-CN.md 译本）
│   ├── development.md
│   ├── contracts/      ← 公开契约（各配 .zh-CN.md 译本）
│   ├── agents/
│   └── solutions/
├── plans/
│   ├── README.md
│   ├── product-definition.md
│   ├── demo-document-catalog.md
│   └── adr/
│       ├── README.md
│       └── 0001-...md
└── research/
    └── README.md
```

## 使用词汇表的词汇

当输出要命名领域概念（issue 标题、重构提案、假设、测试名）时，使用 `CONTEXT.md` 中的定义，不要漂移到词汇表明确列在 `_Avoid_` 下的同义词。

如果你需要的概念还不在词汇表里，这是一个信号：要么你在发明项目并不使用的语言（重新考虑），要么确实存在缺口（记下来交给 `/domain-modeling`）。

## 标记 ADR 冲突

如果输出与现有 ADR 矛盾，明确指出来，而不是静默覆盖：

> _与 ADR-0007（项目命名）矛盾 —— 但值得重新讨论，因为……_

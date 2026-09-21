# 文档索引

本目录存放 Stunt Double 的运维、契约与 agent 文档。本页面向维护者；对外入口是 [GitHub Pages 首页](index.md) 与根 [README](../README.md)。

## 从这里开始

- [开发与发布流程](development.md) — Git 工作流、CI、版本号、发布流程、MSRV 与依赖政策。
- [公开契约](contracts/) — 配置、`ctx` API 与 CLI 契约。
- [Agent 文档](agents/) — issue tracker、triage labels 与领域文档消费规则。
- [GitHub Pages 首页](index.md) — 对外入口页。

## 其他文档

- [README](../README.md) — 项目概览与当前状态。
- [中文 README](../README.zh-CN.md) — 中文项目说明。
- [产品功能定义](../plans/product-definition.md) — v1 范围与不做清单。
- [演示夹具](../demo/README.md) — 可运行的清单场景、上游覆盖、契约与错误映射。
- [架构决策](../plans/adr/) — ADR 0001–0013。
- [调研](../research/) — mock server 生态、运行时选型与开源基线。
- [解决方案](solutions/) — 经 ce-compound 沉淀的经验与已解决问题。
- [`ctx` API 类型定义](../types/ctx-api-v1.d.ts) — `apiVersion` 1 源码类型；T8 随发布产物一同发布。
- [治理](../GOVERNANCE.md) — 维护者模型与响应预期。
- [安全政策](../SECURITY.md) — 漏洞报告与安全预期。
- [变更日志](../CHANGELOG.md) — 发布历史。
- [Agent 指令](../AGENTS.md) — 面向编码代理的仓库规则。

## 文档规则

文档语言与双语结构见 [development.md](development.md) 与 [ADR 0013](../plans/adr/0013-documentation-language-and-bilingual-structure.md)。其余规则：

- 公开契约变更必须同步更新 `docs/contracts/` 与 `CHANGELOG.md`。
- 领域术语变化同步更新 `CONTEXT.md`。
- 难以逆转的决策写入 `plans/adr/`。

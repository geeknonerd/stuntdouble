# 文档语言分层与双语结构

- 状态：`已接受`
- 日期：2026-09-21
- 关联：[ADR 0010](0010-git-and-release-workflow.md)、[CONTRIBUTING.md](../../CONTRIBUTING.md)、[docs/development.md](../../docs/development.md)

## 背景

仓库的主要开发者以中文工作，对外又要以英文默认展示。此前唯一成文的规则是“根目录社区文档使用英文，设计文档可以使用中文”，它重复写在 `CONTRIBUTING.md`、`AGENTS.md`、`docs/README.md` 和 `docs/development.md` 四处，且没有覆盖公开契约、Pages 首页、`solutions`、`agents` 等实际存在的文档类别。结果是 ADR 0001–0007 用中文、0008–0012 用英文，研究笔记用中文而索引用英文，英文 README 直接把英文读者送进中文设计文档。

## 决策依据

1. 读者决定语言：照文档写配置和脚本的外部使用者需要英文；只有维护者自己读的开发文档用中文即可，翻译收益为零。
2. 对外承诺必须英文权威：README、Pages 首页、公开契约是项目的门面与承诺，中文只能是译本，否则英文读者拿到的是二手信息。
3. 双语维护成本只能覆盖小块：每篇双语都是双份维护，只有“入口 + 使用说明 + 契约”值得，全部设计文档不值得也不必。

## 决策

文档语言按读者分三层：

- **A 层（英文单语）**：`CONTRIBUTING.md`、`GOVERNANCE.md`、`SECURITY.md`、`CODE_OF_CONDUCT.md`、`CHANGELOG.md`、`.github/` 模板、commit 与 PR 标题。
- **B 层（英文权威 + `.zh-CN.md` 译本）**：`README.md`、`docs/index.md`、`docs/guide/**`、`docs/contracts/**`、`demo/README.md`。
- **C 层（中文单语）**：`AGENTS.md`、`CONTEXT.md`、`plans/**`、`research/**`、`docs/development.md`、`docs/agents/**`、`docs/solutions/**`。领域术语保留 `CONTEXT.md` 的英文词条，不造中文译名。

双语规则：英文文件不带语言后缀，中文译本用 `.zh-CN.md` 后缀放在同一目录，两页都在标题下方放一行语言切换；英文是唯一权威版本，中文允许滞后但不得与英文矛盾；外部贡献者只提交英文，不因缺少译本被阻塞合并；每次发布前由维护者核对 B 层译本，补不上就删除对应中文页；出现第三种语言、B 层超过 10 篇、或引入站点生成器时，迁移到 `docs/<lang>/` 目录布局。

校验由 `docs-links` CI 门槛承担：`lychee --offline` 检查全部 Markdown 的本地链接，脚本检查双语配对与语言切换链接。外链与译文内容漂移不自动检查，留给发布前人工核对。

## 后果

**正面**：语言规则只有一处权威描述（`docs/development.md`，`CONTRIBUTING.md` 保留面向外部贡献者的摘要），新文档有明确归类；英文读者不再从中文页断层；开发文档不再承担翻译成本。

**负面（接受）**：`docs/` 同时是 GitHub Pages 发布根，中文开发文档会出现在公开站点上，靠导航隔离而不是搬家解决；B 层译文滞后只能靠发布前人工核对发现。

**约束**：新增文档必须能归入 A/B/C 三层之一；B 层新增一篇就要同时提供英文与中文两份，否则不成立。

## 替代方案

- 全英文（拒绝：主要开发者阅读成本高，设计讨论质量下降）。
- 全中文（拒绝：GitHub、crates.io 的默认展示与外部贡献门槛）。
- `docs/en/` + `docs/zh-CN/` 语言目录（拒绝：英文路径被迫带 `/en/` 或依赖根级重定向，当前只有两种语言，收益为零；记为升级路径）。
- mdBook / Docusaurus i18n（拒绝：新增依赖，GitHub Pages 现为 legacy Jekyll 构建，不支持）。
- 全部文档双语（拒绝：维护成本翻倍，设计文档没有英文读者）。

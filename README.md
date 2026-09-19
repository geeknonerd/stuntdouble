> 本目录是 Stunt Double（`stuntdouble`）的公开实现仓库。
> 功能定义、架构决策与演示场景文档由本仓维护，支持开源贡献与社区协作。

# Stunt Double（`stuntdouble`）

> **A test double that plays the whole show.**
> 面向集成联调的 Mock Server：会读外部数据、能吐文件、内置 JS/Python 运行时，零外部环境依赖。

## 项目目标

自研一个可对外发布的 Mock Server 产品（Rust 实现，运行时内置），覆盖声明式路由配置、数据源驱动响应、脚本化变换与文件读写；同时保留历史公开演示场景的实现契约作为需求证据。

## 当前状态

`探索中`

产品功能边界收敛中，尚未进入实现选型。本仓维护产品规划、架构决策与公开演示场景示例。

## 当前文档

- [Mock 服务选型调研](research/Mock服务选型调研.md)：WireMock、Mountebank、MockServer、Mockoon 等方案的能力矩阵、差距与选型建议，作为产品差异化的输入。
- [json-server 外部数据能力](research/json-server外部数据能力.md)：json-server 对外部接口和文件透传能力的边界。
- [示例文档清单与二进制下载场景](plans/demo-document-catalog.md)：manifest 与 PDF 下载接口的契约、实现流程和运行说明，是当前已验证的公开演示场景。
- [Mock Server 产品功能定义](plans/Mock产品功能定义.md)：产品命题、已确认范围与待定事项，随讨论更新。
- [ADR 0001 第一版不支持共享状态](plans/adr/0001-第一版不支持共享状态.md)：第一版数据源边界与已知功能缺口的决策依据。
- [ADR 0002 第一版只实装路由模型](plans/adr/0002-第一版只实装路由模型.md)：单一执行模型与资源派生推迟的决策依据。
- [ADR 0003 完全脚本化与多语言运行时](plans/adr/0003-第一版采用完全脚本化与多语言运行时.md)：变换表达力路线 C、Boa+RustPython 嵌入式 + 零依赖的决策依据（含实验风险）。
- [ADR 0004 脚本能力只经宿主函数提供](plans/adr/0004-脚本能力只经宿主函数提供.md)：半托管沙箱与宿主 API 契约的决策依据。
- [ADR 0005 上游失败语义与可观测性](plans/adr/0005-上游失败语义与可观测性.md)：透传/错误码分界、超时预算、重试与日志策略的决策依据。
- [ADR 0006 产品定位](plans/adr/0006-产品定位.md)：主定位（对接真实外部依赖的联调假服务）、顺风场景与不做范围的决策依据。
- [ADR 0007 项目命名与品牌](plans/adr/0007-项目命名与品牌.md)：Stunt Double / stuntdouble 命名依据、空间核验与演进说明。
- [脚本运行时选型调研](research/脚本运行时选型调研.md)：验证 Boa v0.22.x (test262 >90%) + boa_runtime WebAPI + RustPython stdlib 子集；被否掉的方案说明。
- [架构设计最佳实践调研](research/架构设计最佳实践调研.md)：静态配置/热重载/管理 API 三种模式的对比与演进约束。

## 未决事项

- 配置文件主格式（TOML / YAML / JSON 择一为主）。
- 实现代码的正式仓库位置仍需补充；本仓库只维护调研与实现文档。
- 落地前需按实际安装版本复核 json-server API 与上游依赖差异。

## 最近整理

2026-09-19

# 调研

本目录存放用于产品与架构决策的调研笔记与证据。

| 文档 | 目的 |
| --- | --- |
| [Mock 服务选型调研](mock-server-landscape.md) | 对比 WireMock、Mountebank、MockServer、Mockoon、Prism、json-server 及相关工具。 |
| [json-server 外部数据](json-server-external-data.md) | json-server 外部 HTTP 与文件透传的能力边界。 |
| [脚本运行时选型](script-runtime-selection.md) | Boa 与 RustPython 评估，以及被否决的替代方案。 |
| [架构设计最佳实践](architecture-best-practices.md) | 静态配置、热重载与 Admin API 的取舍。 |
| [开源仓库基线](open-source-repo-baseline.md) | 同类项目的许可证、资金来源与社区文件约定。 |

## 调研规则

- 优先使用一手来源，并给出链接。
- 证据与建议分开陈述。
- 记录日期，以及它影响了哪个决策。
- 不得包含客户名称、内网地址、凭证或生产数据。

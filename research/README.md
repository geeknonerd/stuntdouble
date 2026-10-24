# 调研

本目录存放用于产品与架构决策的调研笔记与证据。

| 文档 | 目的 |
| --- | --- |
| [Mock 服务选型调研](mock-server-landscape.md) | 对比 WireMock、Mountebank、MockServer、Mockoon、Prism、json-server 及相关工具。 |
| [json-server 外部数据](json-server-external-data.md) | json-server 外部 HTTP 与文件透传的能力边界。 |
| [脚本运行时选型](script-runtime-selection.md) | Boa 与 RustPython 评估，以及被否决的替代方案。 |
| [架构设计最佳实践](architecture-best-practices.md) | 静态配置、热重载与 Admin API 的取舍。 |
| [开源仓库基线](open-source-repo-baseline.md) | 同类项目的许可证、资金来源与社区文件约定。 |
| [进程隔离外部事实](process-isolation-external-facts.md) | 核查 rlimit、Windows Job Object、macOS RLIMIT_AS、cargo-dist 多 binary 与父进程退出联动事实。 |
| [进程隔离方案评估](process-isolation-assessment.md) | 整理已定决策与改造量级评估，记录轻量替代方案检索结论（引擎路线、进程库、Wasm/VM、最佳实践）。 |
| [Shared state worker IPC spike](spikes/175-shared-state-ipc/README.md) | 一次性原型验证 worker IPC 上的共享状态原子性、TTL、容量、重启与超时语义（#175；正式实现 #176）。 |
| [cargo-dist CI 硬化](44-dist-ci-hardening.md) | 评估 #44 的权限与 installer 信任链，记录上游状态、行业实践、方案矩阵与下一步建议。 |
| [cargo-dist bootstrap 后续评估](44-bootstrap-followup-evaluation.md) | 复核 #44 第一阶段改动的 pin 边界、跨平台解析与分阶段落地条件。 |

## 调研规则

- 优先使用一手来源，并给出链接。
- 证据与建议分开陈述。
- 记录日期，以及它影响了哪个决策。
- 不得包含客户名称、内网地址、凭证或生产数据。

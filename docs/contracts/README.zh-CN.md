# 公开契约

[English](./README.md) \| **中文**

> 本页是英文版 [README.md](./README.md) 的译本；如有出入，以英文版为准。

Stunt Double 暴露三个公开契约。它们各自演进，在此统一跟踪。

| 契约 | 状态 | 适用 | 稳定性 |
| --- | --- | --- | --- |
| [配置](config.zh-CN.md) | v1 切片已冻结；增量更新至 T12 | v0.x | 1.0 之前允许带弃用窗口的破坏性变更 |
| [`ctx` API](ctx-api.zh-CN.md) | v1 切片已公开；已实现子集至 T12 | `apiVersion` 1 | 同一 `apiVersion` 内只做增量；移除需要新版本 |
| [CLI](cli.zh-CN.md) | v1 切片已冻结；增量更新至 T12 | v0.x | 1.0 之前允许带弃用窗口的破坏性变更 |

## 变更流程

1. 实现所在的 pull request 内同步更新对应契约文件。
2. 在 `CHANGELOG.md` 记录用户可见变化与迁移说明。
3. 移除或改名任何公开面之前，至少提前一个 minor 版本警告。
4. 难以逆转的兼容性决策记录为 `plans/adr/` 下的 ADR。
5. `ctx` API 契约变化时同步更新对应的 `.d.ts`。apiVersion 1 的源码定义在 [`types/ctx-api-v1.d.ts`](../../types/ctx-api-v1.d.ts)；[ADR 0012](../../plans/adr/0012-release-artifacts-and-supply-chain.md) 要求把它纳入发布产物集合。

## 版本规则

- 配置与 CLI 在 1.0 之前跟随产品版本号。
- `ctx` API 使用自己的 `apiVersion`，可以比产品的 major 版本活得更久。
- `1.0.0` 要求配置格式、CLI 与 `ctx` API version 1 稳定。
- 细节见 [ADR 0011](../../plans/adr/0011-contract-compatibility.md)。

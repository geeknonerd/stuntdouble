# 架构决策记录

架构决策按顺序编号，文件名使用 kebab-case。

| ADR | 标题 | 状态 |
| --- | --- | --- |
| [0001](0001-no-shared-state-in-v1.md) | 第一版不支持请求间共享状态 | 已接受 |
| [0002](0002-route-model-only-in-v1.md) | 第一版只实装路由模型，资源派生推迟为同一引擎的预设 | 已接受 |
| [0003](0003-script-first-multi-runtime.md) | 第一版采用完全脚本化，支持 JavaScript/TypeScript + Python 运行时 | 已接受（运行时路径待定） |
| [0004](0004-host-functions-only-sandbox.md) | 脚本能力只经宿主函数提供（半托管沙箱） | 已接受（API 明细待收敛） |
| [0005](0005-upstream-failure-semantics.md) | 上游失败语义：默认透传，传输层失败才造错误码 | 已接受 |
| [0006](0006-product-positioning.md) | 产品定位：对接真实外部依赖的联调假服务 | 已接受 |
| [0007](0007-naming-and-brand.md) | 项目命名与品牌 | 已接受 |
| [0008](0008-dual-mit-apache-license.md) | 核心采用 MIT OR Apache-2.0 双许可证 | 已接受 |
| [0009](0009-open-core-and-funding.md) | 开源核心与资金来源 | 已接受 |
| [0010](0010-git-and-release-workflow.md) | 主干开发与发布流程 | 已接受 |
| [0011](0011-contract-compatibility.md) | 配置、`ctx` API 与 CLI 各自独立兼容契约 | 已接受 |
| [0012](0012-release-artifacts-and-supply-chain.md) | 可验证的发布产物与供应链基线 | 已接受 |
| [0013](0013-documentation-language-and-bilingual-structure.md) | 文档语言分层与双语结构 | 已接受 |

## 如何新增 ADR

使用下一个编号与 kebab-case 文件名，例如 `0014-admin-api-authentication.md`。每条记录只聚焦一个决策，并链接相关 issue 或 ADR。

## 何时需要 ADR

当决策同时满足难以逆转、脱离上下文会令人意外、且源于真实取舍时，写 ADR。决策容易逆转或没有实际替代方案时不必写。

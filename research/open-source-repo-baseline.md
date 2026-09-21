# 开源仓库基线

- 日期：2026-09-19
- 目的：选择许可证与资金来源模型，并让仓库对齐常见开源项目约定。

## 同类项目与许可证

| 项目 | 许可证 | 说明 |
| --- | --- | --- |
| [Rust](https://github.com/rust-lang/rust) | Apache-2.0 | 系统语言，采用对企业友好的许可证并提供显式专利授权。 |
| [clap](https://github.com/clap-rs/clap) | Apache-2.0 | 被广泛商用的 Rust CLI 库。 |
| [bat](https://github.com/sharkdp/bat) | Apache-2.0 | 采用宽松许可证的 Rust CLI 工具。 |
| [Biome](https://github.com/biomejs/biome) | Apache-2.0 | 采用宽松许可证的 Rust 开发工具。 |
| [Tokio](https://github.com/tokio-rs/tokio) | MIT | 采用简短宽松许可证的 Rust 异步运行时。 |
| [Deno](https://github.com/denoland/deno) | MIT | 采用简短宽松许可证的 JavaScript 运行时。 |
| [WireMock](https://github.com/wiremock/wiremock) | Apache-2.0 | 带商业托管产品 WireMock Cloud 的 mock server。 |
| [Mockoon](https://github.com/mockoon/mockoon) | MIT | 带托管产品 Mockoon Cloud 与赞助计划的 mock server。 |
| [Prism](https://github.com/stoplightio/prism) | Apache-2.0 | Stoplight 商业平台内的 API mock 与校验工具。 |
| [json-server](https://github.com/typicode/json-server) | MIT | 采用量很大的极简 mock 数据服务。 |

## 许可证结论

对一个希望被广泛采用、并可能有托管或企业业务的 Rust 开发工具，最稳的默认选择是 **MIT OR Apache-2.0**。

- MIT 给使用者和下游发行版留出最短的宽松路径。
- Apache-2.0 增加了显式专利授权与企业友好的措辞。
- 这一组合符合 Rust 生态预期。
- AGPL-3.0 会给本地/CI 工具带来采用摩擦，不符合当前产品策略。

记录于 [ADR 0008](../plans/adr/0008-dual-mit-apache-license.md)。

## 资金来源结论

对基础设施类工具，单靠捐赠通常不是可持续的资金模式。同类项目中的可行选项是：

1. 围绕开源引擎提供托管服务。
2. 支持合同与 SLA。
3. 企业治理与协作功能。
4. 培训与集成服务。
5. 赞助与捐赠作为补充收入。

记录于 [ADR 0009](../plans/adr/0009-open-core-and-funding.md)。

## 值得采纳的仓库约定

GitHub 的 community profile 建议公开仓库包含这些文件：

- `README.md`
- `LICENSE` 文件
- `CONTRIBUTING.md`
- `CODE_OF_CONDUCT.md`（存在报告联系人时）
- `SECURITY.md`
- issue 模板
- pull request 模板

本仓库现在包含 README（英文与中文）、双许可证文件、CONTRIBUTING、CODE_OF_CONDUCT、SECURITY、issue 模板与 pull request 模板。在专门的 conduct 邮箱出现之前，行为准则使用仓库的私密报告表单。

## 来源

- GitHub Docs，About community profiles for public repositories：https://docs.github.com/en/communities/setting-up-your-project-for-healthy-contributions/about-community-profiles-for-public-repositories
- GitHub Docs，Licensing a repository：https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/licensing-a-repository
- Choose a License，MIT：https://choosealicense.com/licenses/mit/
- Choose a License，Apache-2.0：https://choosealicense.com/licenses/apache-2.0/
- WireMock Cloud：https://www.wiremock.io/
- Mockoon Cloud：https://mockoon.com/cloud/
- Open Source Guides，Starting an Open Source Project：https://opensource.guide/starting-a-project/

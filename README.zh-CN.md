# Stunt Double

> **A test double that plays the whole show.**

Stunt Double 是一个用 Rust 实现的 Mock Server，面向需要对接真实外部依赖的集成测试。它会读取上游接口、用内置 JavaScript 或 Python 变换数据、返回文件与二进制响应，并且不依赖宿主机上的 Node.js、Python 或 JVM。

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#许可证)
[![Status: upstream HTTP](https://img.shields.io/badge/status-upstream%20http-orange.svg)](#当前状态)

## 为什么做 Stunt Double

多数 Mock 工具擅长静态桩，Stunt Double 针对静态桩通常覆盖不了的集成工作：

- 需要从真实上游接口取数。
- 需要 CSV、JSON、文本或二进制变换。
- 需要 PDF、文件或字节流响应，并支持 Range。
- CI 环境不能安装 Node.js、Python 或 JVM。
- 需要脚本行为，但必须有明确的宿主限制，而不是不受限的运行时。

设计目标是单个二进制文件尽可能贴近真实依赖的行为，让客户端测到真实链路，而不只是拿到一份固定响应。

## 当前状态

**脚本执行与上游 HTTP（T4）已落地。** `stuntdouble serve` 与 `stuntdouble validate` 读取 TOML 配置，按方法 + 路径匹配路由，并用内置 Boa 执行路由 JavaScript，宿主注入的 `ctx` 提供 `apiVersion` / `request` / `http.get` / `respond` / `log` / `env`。`ctx.http.get` 只能访问 `[upstream] allow_hosts` 列出的 host，上游 4xx/5xx 视为数据，未捕获的传输层失败映射为 502 `upstream_unreachable`。未命中路由返回 404 `not_found`；脚本异常或超时返回 500 `script_error`，未调用 `ctx.respond` 返回 500 `script_no_response`，响应均带 `request_id`。文档清单场景已提供可运行夹具 [demo/](demo/README.md)：经 `ctx.http.get` 读取上游元数据并返回 CSV 清单。文件与二进制响应待后续切片。见 [plans/adr/](plans/adr/)。

## v1 计划范围

- 路由模型只有一条流水线：`match → source → transform → response`。
- 内置 JavaScript 运行时：Boa。
- 内置 Python 运行时：RustPython stdlib 子集。
- 宿主注入 `ctx` API，不暴露裸 `fetch`、`fs`、`os`、`subprocess`、`socket`。
- 上游 HTTP（`ctx.http.get` 已实现，request/pipe 待后续切片）、静态文件读取、文件流、上传、Range 响应。
- 唯一静态文件根，并阻止路径穿越。
- 静态配置 + 重启。热重载与 Admin API 推迟。
- Linux x86_64、macOS arm64、Windows x86_64 二进制与容器镜像。
- 结构化请求日志，包含 `request_id` 与稳定错误分类。

## v1 不做

- 请求间共享状态。
- 响应推进。
- 自动资源 CRUD。
- TypeScript 转译。
- npm、pip 或第三方导入。
- 热重载、Admin API、GUI。
- 内置 TLS 终止。
- WebSocket、GraphQL、gRPC。

完整范围见 [plans/product-definition.md](plans/product-definition.md)。

## 文档

详细流程与契约见 [docs/README.md](docs/README.md)。

- [产品功能定义](plans/product-definition.md)
- [公开演示场景](plans/demo-document-catalog.md)
- [演示夹具](demo/README.md)
- [架构决策](plans/adr/)
- [开发和发布流程](docs/development.md)
- [治理规范](GOVERNANCE.md)
- [公开契约](docs/contracts/)
- [领域词汇表](CONTEXT.md)
- [变更日志](CHANGELOG.md)
- [文档索引](docs/README.md)
- [English README](README.md)

## 参与贡献

提出功能前先阅读产品定义与 ADR。v1 范围刻意收窄，新能力必须落在单一执行模型内。

- 遵守 [Code of Conduct](CODE_OF_CONDUCT.md)。
- Bug 与功能请求使用 GitHub issue 模板。
- 客户数据不得进入 issue、日志、fixture 或截图。
- 提交使用 `git commit -s`（DCO）。
- commit message 使用英文。

完整流程见 [CONTRIBUTING.md](CONTRIBUTING.md) 与 [docs/development.md](docs/development.md)。

## 安全

不要在公开 issue 中报告漏洞。请使用本仓库的 GitHub 私密漏洞报告功能，见 [SECURITY.md](SECURITY.md)。

公开 issue 中禁止粘贴客户主机名、Token、Header、生产日志、请求体或响应体。

## 许可证

双许可证，任选其一：

- Apache License, Version 2.0（[LICENSE-APACHE](LICENSE-APACHE)）
- MIT（[LICENSE-MIT](LICENSE-MIT)）

这是 Rust 项目常见的宽松许可证组合，便于商业与开源环境采用。

除非明确声明，否则任何有意提交并纳入本项目的贡献都按上述双许可证授权，不附加额外条款。

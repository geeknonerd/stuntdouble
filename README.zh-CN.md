# Stunt Double

[English](README.md) | **中文**

> 本页是英文版 [README.md](README.md) 的译本；如有出入，以英文版为准。

> **A test double that plays the whole show.**

Stunt Double 是一个用 Rust 实现的 Mock Server，面向需要对接真实外部依赖的集成测试。它会读取上游接口、用内置 JavaScript 变换数据、返回文件与二进制响应，并且不依赖宿主机上的 Node.js、Python 或 JVM。

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#许可证)
[![Release](https://img.shields.io/github/v/release/geeknonerd/stuntdouble)](https://github.com/geeknonerd/stuntdouble/releases)
[![CI](https://github.com/geeknonerd/stuntdouble/actions/workflows/ci.yml/badge.svg)](https://github.com/geeknonerd/stuntdouble/actions/workflows/ci.yml)

v1.0.1 已发布，配置、`ctx` API、CLI 三契约已冻结。执行模型与安全边界见 [plans/adr/](plans/adr/)，细节见 [docs/](docs/README.md)。

## 快速开始（60 秒）

从 checkout 直接跑离线 demo——无需新建文件，无需联网：

```bash
cargo run -- serve --config demo/stuntdouble.toml
curl -i http://127.0.0.1:3000/demo/documents/local-manifest/group-a
```

期望返回 `200` 与 CSV：

```text
文件编码,文件标题,系统代码
DOC-0001,示例设备 A 安装手册,SYS-A
DOC-0002,示例设备 B 运行手册,SYS-B
```

下一步：[快速开始](docs/guide/getting-started.zh-CN.md) 从零建第一条路由；[Mock 场景示例](docs/guide/mock-recipes.zh-CN.md) 覆盖上游、文件与上传流程。

## 为什么做 Stunt Double

多数 Mock 工具擅长静态桩，Stunt Double 针对静态桩覆盖不了的集成工作：

- 需要从真实上游接口取数。
- 需要经脚本做 CSV、JSON、文本或二进制变换。
- 需要 PDF、文件或字节流响应，并支持 Range。
- CI 环境不能安装 Node.js、Python 或 JVM。

| 工具 | 一句话定位 | 何时选 Stunt Double |
|---|---|---|
| WireMock | Java 系通用 stub | 需要 Rust 单二进制 + 上游改写 + 二进制流 |
| Mockoon | 本地静态 stub 与 GUI | 需要脚本变换 + Range/206 + 零运行时 CI |
| json-server | 资源派生 CRUD | 需要逐条 Route 声明 + 上游 + 文件根 |
| Prism | OpenAPI 契约校验 | 需要“先改真数据再返回”的替身 |

## 安装

从 [releases 页面](https://github.com/geeknonerd/stuntdouble/releases)下载对应平台归档，再验证构建来源：

```bash
gh attestation verify stuntdouble-x86_64-unknown-linux-gnu.tar.gz --repo geeknonerd/stuntdouble
```

或跑容器镜像（挂载配置里设置 `server.bind = "0.0.0.0"`）：

```bash
docker run --rm -p 8080:8080 \
  -v "$PWD/stuntdouble.toml:/etc/stuntdouble/stuntdouble.toml:ro" \
  ghcr.io/geeknonerd/stuntdouble:v1.0.1
```

贡献者用 `cargo install --path .` 从源码安装；完整资产清单与发布检查表见 [docs/development.md](docs/development.md)。

## 功能

- 路由模型只有一条流水线：`match → source → transform → response`。
- 内置 JavaScript 运行时（Boa）与宿主注入的 `ctx`；不暴露裸 `fetch`、`fs`、`os`、`subprocess`、`socket`。
- 上游 HTTP（`ctx.http.get`、支持 Range 透传的流式 `ctx.http.pipe`）与 `ctx.file` 读取、文件流、请求级 multipart 上传。
- 唯一静态文件根并阻止路径穿越；静态配置 + 重启生效。
- Linux x86_64、macOS arm64、Windows x86_64 二进制与 `linux/amd64`、`linux/arm64` 双平台容器镜像。
- 结构化请求日志，含 `request_id`、上游调用链与稳定错误分类。

## v1 不做（1.0.0 范围外）

- 请求间共享状态；响应推进；自动资源 CRUD。
- TypeScript 转译；Python 运行时（v2，见 ADR 0015）；npm、pip 或第三方导入。
- 热重载、Admin API、GUI；内置 TLS 终止。
- WebSocket、GraphQL、gRPC。
- `ctx.http.request`、`retries`、`bodyBytes`（已延期，见 ADR 0016）。

完整范围见 [plans/product-definition.md](plans/product-definition.md)。

## 文档

- [快速开始](docs/guide/getting-started.zh-CN.md)——安装、第一条路由与上游调用。
- [Mock 场景示例](docs/guide/mock-recipes.zh-CN.md)——按任务组织的 JSON、上游、文件、上传与 CI 示例。
- [公开契约](docs/contracts/README.zh-CN.md)——配置、`ctx` API 与 CLI。
- [`ctx` API 类型定义](types/ctx-api-v1.d.ts)——apiVersion 1 源码类型。
- [演示夹具](demo/README.zh-CN.md)——可运行的离线与上游文档场景。
- [文档站](https://geeknonerd.github.io/stuntdouble/)——渲染后的契约与 demo 入口。
- [变更日志](CHANGELOG.md)——发布历史。

开发文档为中文；[ADR 0013](plans/adr/0013-documentation-language-and-bilingual-structure.md) 记录语言策略。

- [产品功能定义](plans/product-definition.md)（中文）——v1 范围、宿主 API 与不做清单。
- [开发与发布流程](docs/development.md)（中文）——分支、提交、CI、版本与发布规则。
- [架构决策](plans/adr/)（中文）——ADR 0001–0017。
- [领域词汇表](CONTEXT.md)（中文）——项目词汇。
- [调研](research/)（中文）——Mock 选型、运行时、架构与开源基线。
- [仓库文档索引](docs/README.md)（中文）——维护者入口。

## 参与贡献

提功能前先读产品定义与 ADR；新能力仍须落在单一执行模型内。遵守 [Code of Conduct](CODE_OF_CONDUCT.md)，用 GitHub issue 模板，客户数据不得进入 issue 与 fixture，提交用 `git commit -s`（DCO），commit message 用英文。见 [CONTRIBUTING.md](CONTRIBUTING.md) 与 [docs/development.md](docs/development.md)。

## 安全

不要在公开 issue 中报告漏洞，请用本仓库的 GitHub 私密漏洞报告，见 [SECURITY.md](SECURITY.md)。公开 issue 禁止粘贴客户主机名、Token、Header、生产日志、请求体或响应体。

## 许可证

双许可证任选其一：[Apache-2.0](LICENSE-APACHE) 或 [MIT](LICENSE-MIT)。除非明确声明，任何有意提交并纳入本项目的贡献默认按上述双许可证授权，不附加额外条款。

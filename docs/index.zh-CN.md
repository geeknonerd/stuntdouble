---
title: Stunt Double
---

# Stunt Double

[English](./index.md) \| **中文**

> 本页是英文版 [index.md](./index.md) 的译本；如有出入，以英文版为准。

**A test double that plays the whole show.**

Stunt Double 是一个用 Rust 实现的 mock server，面向需要对接真实外部依赖的集成测试。它会读取上游接口、用内置 JavaScript 变换数据、返回文件与二进制响应，并且不依赖宿主机上的运行时。

## 状态

T1–T8 切片已完成：T1–T7 实现运行时，T8 冻结 v1 配置、CLI 与 `ctx` API 契约，并定义公开的 `apiVersion` 1 类型定义。`stuntdouble serve` 与 `stuntdouble validate` 从 TOML 配置启动；命中的 Route 在内置 Boa 运行时中执行 JavaScript，宿主注入 `ctx`（`apiVersion`、`request`、`http.get`、`http.pipe`、`respond`、`log`、`env`）；`ctx.http.get` 执行 allowlist 约束的上游 HTTP 调用，`ctx.http.pipe` 把 allowlist 内的上游 body 连同 Range 透传流给客户端。未命中的 Route 返回 404 `not_found`；脚本失败返回 500 `script_error` 或 `script_no_response`；未捕获的上游传输层失败返回 502 `upstream_unreachable`。命中的脚本还受宿主资源边界约束（应答时限、循环次数兜底、递归/VM 栈上限），引擎 panic 返回 500 `script_error`；Boa 0.22 不暴露堆指标或 interrupt 钩子，进程内堆上限属于已记录的接受风险（[SECURITY.md](https://github.com/geeknonerd/stuntdouble/blob/main/SECURITY.md)）。文档清单与 PDF 下载演示场景由仓库内的 `demo/` 夹具驱动。每个请求还会写出一条结构化日志，包含上游调用链、body 大小与白名单 header；`serve --verbose` 为 500/502 响应附加稳定的 `detail` 类别，不暴露堆栈、上游 body 或内部地址。本地静态文件、上传与文件响应是后续切片。

## 文档

- [README](https://github.com/geeknonerd/stuntdouble/blob/main/README.md) — 项目概览与当前状态。
- [快速开始](https://github.com/geeknonerd/stuntdouble/blob/main/docs/guide/getting-started.zh-CN.md) — 最小可用配置与第一个路由。
- [公开契约](https://github.com/geeknonerd/stuntdouble/tree/main/docs/contracts) — 配置、`ctx` API 与 CLI。
- [演示夹具](https://github.com/geeknonerd/stuntdouble/blob/main/demo/README.zh-CN.md) — 可运行的清单与 PDF 下载场景。
- [`ctx` API 类型定义](https://github.com/geeknonerd/stuntdouble/blob/main/types/ctx-api-v1.d.ts) — `apiVersion` 1 源码类型。
- [变更日志](https://github.com/geeknonerd/stuntdouble/blob/main/CHANGELOG.md) — 发布历史。
- [参与贡献](https://github.com/geeknonerd/stuntdouble/blob/main/CONTRIBUTING.md)
- [安全政策](https://github.com/geeknonerd/stuntdouble/blob/main/SECURITY.md)

## 许可证

双许可证 MIT OR Apache-2.0，任选其一。

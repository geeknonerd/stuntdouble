# 可验证的发布产物与供应链基线

- 状态：`已接受`
- 日期：2026-09-19
- 关联：[ADR 0008](0008-dual-mit-apache-license.md)、[ADR 0010](0010-git-and-release-workflow.md)、[开发指南](../../docs/development.md)

## 背景

Stunt Double 面向 CI 与本地开发，使用者需要可信的二进制与容器。无法追溯到 tag、或下载后无法验证的发布，会带来本可避免的供应链风险。

## 决策

每个 release 提供可验证的产物集合：

- GitHub 生成的源码归档
- Linux x86_64、macOS arm64、Windows x86_64 二进制
- 每个受支持 `apiVersion` 的 `ctx` API `.d.ts` 类型定义
- `SHA256SUMS`
- GitHub artifact attestation
- CycloneDX 或 SPDX 格式的 SBOM
- 容器镜像 `ghcr.io/geeknonerd/stuntdouble:<tag>`，并公布 digest
- 由 CHANGELOG 生成的 GitHub Release notes

只从 tag 构建产物，绝不发布从 `main` 构建的二进制。

签名从 GitHub artifact attestation 开始；只有出现离线验证需求时才引入 Sigstore/cosign。

容器镜像使用多阶段构建与最小运行时镜像。tag 与 digest 都要发布；`latest` 绝不作为唯一引用。

发布失败不覆盖已有产物，修复问题后发布新版本。只有 crates.io 发布损坏时才用 `cargo yank`。

## 工具

- `release-plz`：release PR、版本号、CHANGELOG、tag 与可选的 crates.io 发布。
- `cargo-dist`：多平台二进制、安装器、校验和与 GitHub Release 资产。
- GitHub Actions：CI 与发布流程。
- `cargo-deny` 与 `cargo-audit`：依赖与安全公告检查。
- Dependabot：Cargo 与 GitHub Actions 更新。

## 启用时机

发布流程与 `cargo-dist` 配置在第一个实现切片中加入，早于首次公开发布。在此之前，本 ADR 定义的是必须具备的形态，而不是已启用的流水线。

## 后果

- 发布自动化需要访问仓库与包注册表的 secret。
- 产物可以从 tag 与锁定的依赖图复现。
- release notes 是发布产物的一部分，不是事后补充。

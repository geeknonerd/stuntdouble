---
title: "Cross-compile the linux/arm64 release container image on an amd64-only runner"
date: 2026-09-24
category: ci
module: multi-platform release image
problem_type: tooling_decision
component: ci
applies_when:
  - "在只有 amd64 的 runner 上，用一个 tag 同时发布 `linux/amd64` 与 `linux/arm64` 容器镜像时"
  - "在「全量 QEMU 模拟」与「宿主平台 builder 交叉编译」之间为 arm64 选择构建路径时"
  - "在 Docker 构建里为 `rust-toolchain.toml` 固定 channel 的 Rust 项目添加 cross target 时"
  - "为依赖 C/汇编的 crate（如 `ring`）安装交叉 linker 与目标 libc 头文件时"
  - "校验已推送的 tag 是否是恰好覆盖预期 Linux 架构的镜像索引时"
symptoms:
  - "`ghcr.io/geeknonerd/stuntdouble:v0.2.1` 解析为单个 amd64 的 OCI image manifest，arm64 主机只能退回模拟运行"
  - "交叉构建报 `error[E0463]: can't find crate for core`，因为 `rustup target add` 装到了镜像的默认工具链上"
  - "交叉构建报 `bits/libc-header-start.h: No such file or directory`，直到补装 `libc6-dev-arm64-cross`"
root_cause: incomplete_setup
resolution_type: workflow_improvement
severity: medium
tags: [docker-buildx, multi-platform, arm64, cross-compilation, rustup, qemu, ghcr, release-automation]
---

# 在只有 amd64 的 runner 上交叉编译 `linux/arm64` 发布镜像

## 背景（Context）

发布契约把容器镜像与三平台归档并列发布（`docs/development.md:103`）。原来的 `release-extras` 调用 `docker/build-push-action` 时没有传 `platforms`，tag 因此只承载 runner 自身的架构：issue #67 核实发布时的历史 tag `ghcr.io/geeknonerd/stuntdouble:v0.2.1`（registry 引用，不是仓库路径）是单个 amd64 的 OCI image manifest，arm64 主机只能退回模拟运行，而 cargo-dist 早已发布 `aarch64-apple-darwin` 归档。决策与实测记录在 ADR 0012 的 #67 修订（`plans/adr/0012-release-artifacts-and-supply-chain.md:69-77`）。

（session history）更早一轮里 #67 还带着 `needs-triage`，发布链已经在推送 GHCR 镜像并把 digest 写进 `stuntdouble-<version>-image.txt`：「镜像推没推」早有证据源，缺的是「推出去的 tag 是否覆盖两个架构」这层保证。

## 做法（Guidance）

1. **builder 固定在宿主平台，按 `TARGETARCH` 选择本机或交叉目标。** `FROM --platform=$BUILDPLATFORM ...`（`Dockerfile:1`）让 builder 始终原生执行；amd64 分支不装交叉包，arm64 走交叉分支（`Dockerfile:11-25`）。产物统一落到 `/stuntdouble` 再 COPY 进 runtime 阶段（`Dockerfile:50`、`Dockerfile:58`），两个平台共用同一条 COPY 路径。
2. **交叉编译器与目标 libc 头是两件事。** arm64 分支安装 `gcc-aarch64-linux-gnu` 与 `libc6-dev-arm64-cross`（`Dockerfile:16-18`），并用 `CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER` 指向交叉 linker（`Dockerfile:8`）；否则 rustc 仍会调用宿主 `cc`。
3. **`rustup target add` 必须放在 `COPY . .` 之后。** `rust-toolchain.toml` 把 channel 固定为 `stable`（`rust-toolchain.toml:1-3`），rustup 会据此另装并选用一套 `stable` 工具链；在复制源码之前安装 target 会落到镜像原有的默认工具链，cargo 用的却是另一套。当前顺序是 `COPY . .`（`Dockerfile:28`）在前，`rustup target add` 与 `cargo build --target`（`Dockerfile:41`、`Dockerfile:48`）在后。
4. **workflow 传 `platforms`，并在公告前断言 registry 里的原始索引。** `platforms: linux/amd64,linux/arm64`（`.github/workflows/release-extras.yml:148`）；断言读取 raw index 并要求 Linux 架构集合恰为 `amd64 arm64`（`.github/workflows/release-extras.yml:194-205`），`select(.platform.os == "linux")` 让 attestation 的 `unknown/unknown` manifest 不影响判定。
5. **QEMU 只服务于 runtime 阶段。** arm64 的 runtime 阶段仍要跑 `apt-get install ca-certificates`（`Dockerfile:54-56`），所以 workflow 仍需在 Buildx 之前 setup QEMU（`.github/workflows/release-extras.yml:91-95`）；arm64 的 Rust 编译不在模拟下发生，这正是它比全量模拟快一个量级的原因。
6. **基础镜像 pin 必须是索引 digest。** `Dockerfile:1` 与 `Dockerfile:52` 的 `@sha256:` pin 要能按架构解析；两条构建路径都成功即证明它们是索引 digest——若 pin 成按架构的 manifest digest，arm64 会直接解析失败。
7. **不要用「能拉取」代替平台断言。** 旧的发布验证只检查 tag 可拉取与 digest（`docs/development.md:81-82`），发现不了 arm64 主机实际拿到 amd64 manifest；与断言同语义的人工命令及期望输出见 `docs/development.md:83-85`。
8. **旧 tag 保持旧形状。** `v0.2.1` 及更早仍是单平台，重跑它们的 `release-extras` 会在断言处失败，这是「不覆盖已有产物」的预期行为（`docs/development.md:90`）。

## 为什么重要（Why This Matters）

- **平台承诺必须与 registry 产物一致。** tag 与 digest 只证明「有一份镜像」；只有 index 的 manifest 集合能证明 arm64 原生可用。断言因此检查已推送的 raw index，而不是 Dockerfile 或 workflow 里有没有写参数。
- **构建成本决定方案能否长期留在发布链路。** 全量模拟超过 25 分钟仍未完成，交叉编译 arm64 的编译步骤 263 秒，与原生 amd64 的 258 秒基本持平（`plans/adr/0012-release-artifacts-and-supply-chain.md:74`）。
- **rustup 的 toolchain 选择是隐藏状态。** Dockerfile 只差一行的位置，失败却发生在 crate 编译阶段（E0463），排查成本远高于改动本身。
- **交叉编译 C/汇编依赖多一层要求。** `ring` 的报错说明只有编译器还不够，目标 libc 头文件同样必需。

## 何时适用（When to Apply）

- CI runner 只有 amd64，却要发布 `linux/amd64` 与 `linux/arm64` 容器镜像时。
- 在「全量模拟」与「宿主平台 builder 交叉编译」之间做选择时。
- 项目用 `rust-toolchain.toml` 或 rustup channel 固定工具链，且需要在镜像里添加 cross target 时。
- 依赖含 `ring`、OpenSSL、SQLite 等需要目标 C 工具链或 libc 头文件的 crate 时。
- 增加新容器架构、调整 `TARGETARCH` 分支、合并构建阶段，或升级 builder/runtime base image 时。

## 示例（Examples）

**错误 1：target 装错位置。** 在 `COPY . .` 之前执行 `rustup target add aarch64-unknown-linux-gnu`，随后交叉 `cargo build` 报：

```text
error[E0463]: can't find crate for core
  = note: the `aarch64-unknown-linux-gnu` target may not be installed
```

修复即把安装挪到 `COPY . .` 之后（`Dockerfile:41`）。

**错误 2：只装交叉 gcc。** 编译 `ring` 时报：

```text
/usr/include/stdint.h:26:10: fatal error: bits/libc-header-start.h: No such file or directory
```

修复是同时安装 `gcc-aarch64-linux-gnu` 与 `libc6-dev-arm64-cross`（`Dockerfile:16-18`）。补装后交叉 gcc 能产出 aarch64 ELF，两个平台的镜像都能启动并响应 demo 请求。

**发布形状对照：**

```text
修改前：amd64 runner            -> 单平台 image manifest
修改后：$BUILDPLATFORM builder  -> {amd64 原生, arm64 交叉编译} -> 双平台 image index
```

**验证边界：** 本轮在本地完成两平台构建、容器端到端请求（arm64 经 QEMU 执行）与断言桩测试（多平台索引通过、单平台索引 fail closed）；真实的多平台 push 要由下一次 release 首次执行（PR #81，截至本文未合并）。在它成功之前，应表述为「修复已落分支、待 release 验证」，而不是「registry 已修复」。

## 相关（Related）

- `docs/solutions/ci/ghcr-package-visibility-follows-the-publishing-token.md` — 同属 `release-extras`/GHCR 发布面，但根因与解法不同（创建包的 token 路径 vs 平台覆盖）。
- `plans/adr/0012-release-artifacts-and-supply-chain.md` — #67 修订是这一决策的权威记录。
- `docs/development.md` — 发布步骤 7、发布验证配方与旧 tag 边界。
- issue #67、PR #81。

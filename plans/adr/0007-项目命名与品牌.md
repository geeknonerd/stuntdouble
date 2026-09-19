# ADR 0007: 项目命名与品牌

- 状态：`已接受`
- 日期：2026-09-18
- 决策人：项目团队（经过充分讨论与收敛）

## 背景

Mock Server 产品的功能边界（ADR 0001–0006）已收敛，需要从临时占位名 `mock-service` 确定一个正式、可对外发布、具备商业与社区识别度的品牌名称，并将实现目录重命名。

## 调研输入

1. **同类既有命名分布**：
   - 静态桩主流：WireMock、MockServer、Mockoon、Prism、json-server
   - Rust 生态同类：apate、mocktail、mockiapi、rift、httpcan、rustyjsonserver
   - "mock" 前缀高度拥挤，容易与既有工具混淆。
2. **命名空间占用（2026-09-18 实测）**：
   - crates.io `stuntdouble`：空闲
   - npm `stuntdouble`：空闲
   - 域名 `stuntdouble.dev` / `stuntdouble.io`：未注册
   - GitHub 用户 `stuntdouble`：存在休眠老账号（2008 年），组织名不可直接使用同一名称
   - GitHub 组织 `stunt-double`：已被 AI agent 公司占用
   - npm `stunt` / crates.io `stunt`：已被占用，不宜作为短名

## 决策

1. **展示名**：`Stunt Double`
2. **技术标识**：`stuntdouble`（全小写、无连字符，crate / npm / 二进制 / 统一使用此名）
3. **定位口号（Tagline）**：*A test double that plays the whole show.*
   - 中文辅助定位：面向集成联调的 Mock Server：会读外部数据、能吐文件、内置 JS/Python 运行时，零外部环境依赖。
4. **仓库位置**：GitHub 组织 `geeknonerd` 下的 `stuntdouble` 仓库

## 理由

- **隐喻对齐**：产品定位是做外部依赖的"替身"，且能完整演完全场（读真实外部接口、流式吐文件、执行 JS/Python 变换），与"特技替身（stunt double）"的角色同构。
- **命名空间干净**：核心分发渠道 crates.io 与 npm 均全空闲，域名未被抢注。
- **避免撞名**：避开已拥挤的 `mock-*` 前缀，建立独立品牌识别。

## 演进说明

- 配置文件默认名候选：`stuntdouble.toml`（或与最终选定的格式对齐）。
- 二进制产物命名：`stuntdouble`。

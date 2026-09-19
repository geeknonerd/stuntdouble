# Stunt Double 开发约束

## 项目概览

Stunt Double（技术标识 `stuntdouble`）是面向集成联调的 Rust Mock Server：

- 声明式路由配置
- 外部数据源与本地文件驱动响应
- JavaScript / Python 脚本变换
- 单二进制、零外部运行时依赖
- 支持 HTTP 文件流、二进制透传、上传落盘

定位：对接真实外部依赖的联调假服务，主要服务本地开发与 CI。

## 当前仓库状态

- 实现进度：探索中，尚无 Rust 实现代码
- 目录职责：本目录是公开的实现与文档仓库
- `plans/` 与 `research/`：产品规划、架构决策与演示场景文档
- 本仓为开源实现仓库；功能定义与决策以本仓为准
- 文档基线：2026-09-19
- 协作规范：已明确 Git 工作流、版本号、发布、契约与治理规则

## 权威文档

- [README.md](README.md)：项目目标、当前状态、文档索引
- [docs/development.md](docs/development.md)：Git 工作流、版本号、发布流程、MSRV 与依赖规范
- [GOVERNANCE.md](GOVERNANCE.md)：维护者职责与响应预期
- [docs/contracts/](docs/contracts/)：配置、`ctx` API、CLI 的公开契约
- [CONTEXT.md](CONTEXT.md)：项目领域词汇表
- [plans/product-definition.md](plans/product-definition.md)：v1 范围、宿主 API、不做清单
- [plans/demo-document-catalog.md](plans/demo-document-catalog.md)：已验证的公开演示场景接口契约
- [plans/adr/](plans/adr/)：架构决策记录，包含许可证与商业化决策
- [research/](research/)：选型调研与能力边界证据

## v1 核心边界

- Rust 实现 Mock Server 主体
- 内置 Boa v0.22.x 执行 JavaScript
- 内置 RustPython 执行 Python stdlib 子集
- 第一版不支持 TypeScript 转译
- 第一版不支持 `import` / `require` / npm / pip
- 第一版不支持请求间共享状态
- 第一版不实装资源派生模型
- 第一版只做路由模型配置
- 第一版不做热重载、Admin API、GUI、内置 TLS、WebSocket、GraphQL、gRPC

## 单一执行模型

所有接口必须落在同一条流水线内：

```text
match → source → transform → response
```

- `match`：HTTP method、path、params、query 匹配
- `source`：本地静态数据文件或外部上游 HTTP
- `transform`：JS / Python 脚本
- `response`：状态码、headers、body、文件流或透传响应

约束：任何新增能力都不得绕开该模型另起执行路径。

## 脚本沙箱

脚本只允许使用宿主注入的 `ctx` API。引擎原生能力不暴露给脚本。

可用宿主能力：

- `ctx.request`：请求只读上下文
- `ctx.http.get` / `ctx.http.request`：白名单上游 HTTP
- `ctx.http.pipe`：上游响应体流式透传
- `ctx.file.readText` / `ctx.file.readBytes` / `ctx.file.stream`：静态文件目录内只读文件访问
- `ctx.respond`：显式响应
- `ctx.local`：请求内暂存，随请求销毁
- `ctx.log`：服务端日志
- `ctx.env`：环境变量
- `setTimeout` / `setInterval`：受脚本总超时截断

禁止：

- 裸 `fetch`
- Node `fs` / `process`
- Python `os` / `subprocess` / `socket`
- 脚本写文件
- 脚本访问静态文件目录外路径
- 脚本持有跨请求状态

硬限制：

- 脚本总超时默认 10 秒，可配置
- 内存上限约 64MB
- 网络域名白名单
- 文件目录限制
- 上传默认上限 20MB，可配置
- 堆栈只进服务端日志，不进响应

## 上游失败语义

以“是否拿到 HTTP 响应”为唯一分界：

- 上游返回任意 HTTP 响应：视为数据，默认透传状态码；脚本可改写
- 上游传输层失败：抛异常给脚本；未捕获则返回 502 并带 `request_id`
- 脚本异常：返回 500
- 脚本未调用 `respond`：视为逻辑错误，不静默返回 200
- 默认不重试；仅 `opts.retries` 显式开启，上限 3 次，只处理传输层失败

## 文件与响应

- 静态文件目录是唯一文件根
- 脚本文件路径相对该根解析
- 拒绝绝对路径和 `..`
- 上传由宿主解析 multipart，并落到每请求临时目录
- 请求结束清理上传临时目录
- 响应支持字符串、bytes、文件流
- 文件流支持 Range
- 纯内存响应不支持 Range

## 配置

- v1 主路径：改配置文件 + 重启
- 配置文件主格式未定：TOML / YAML / JSON 择一为主
- 预留未来入口：热重载、管理 API、OpenAPI 预设生成器
- 若加入 reload 或 Admin API，必须复用同一个 `reload()` 入口完成路由表原子切换
- Admin API 如暴露，必须独立端口或 localhost，并有基础鉴权

## 已验证场景

`plans/demo-document-catalog.md` 记录了当前已验证的 json-server 公开演示场景。实现产品化能力时，应优先保持该场景契约：

- `GET /demo/documents/manifest/:group`
  - 读取固定元数据接口
  - 输出 UTF-8 CSV 文本
  - 表头固定：`文件编码,文件标题,系统代码`
  - 每行使用 `code,title,system_code`
- `GET /demo/documents/download/:document_id`
  - 按 `code` 精确匹配元数据记录
  - 读取 `pdf_url`
  - 获取并返回 PDF
- 元数据接口非 2xx：502
- 文档不存在：404
- PDF 下载失败：502

## Agent 协作约定

- **先读文档，再动手**：遇到功能问题先看 [Mock 产品功能定义.md](plans/product-definition.md)，代码实现前先理解四段流水线与 `ctx` API 约束
- **修根因**：共享函数的改动优先在源头修改，而非各调用点打补丁
- **简洁表达**：首行给动作；多步编号，每步一事；结尾给出一个明确的下一步时长建议

## 许可证与商业化

- 核心采用 MIT OR Apache-2.0 双许可证，任选其一
- 贡献按 DCO sign-off，不引入 CLA
- 商业化优先级：支持与 SLA、托管团队服务、企业治理、培训与集成服务
- 打赏与赞助仅作为补充，不作为主要收入来源
- 详细决策见 `plans/adr/0008-dual-mit-apache-license.md` 与 `plans/adr/0009-open-core-and-funding.md`

## 开发与发布规范

- 主干开发：`main` 始终可发布，所有变更经短分支 + PR
- 合并策略：仅 squash merge，合并后自动删分支
- 提交：英文 Conventional Commits，必须 `git commit -s`（DCO）
- 分支命名：`feat/`、`fix/`、`docs/`、`chore/`、`ci/`、`release/`
- 版本号：SemVer，tag 为 `vMAJOR.MINOR.PATCH`
- 发布：`release-plz` 生成 release PR，合并后打 tag；`cargo-dist` 构建产物
- CI 硬门槛：`fmt`、`clippy`、`test`、`docs`、`deny`、`audit`、`msrv`、`pr-title`、`dco`
- 兼容性：配置、`ctx` API、CLI 三份契约分开版本化
- `ctx` API：同一 `apiVersion` 只加不删；删除或重命名需要新 `apiVersion`
- 产物：三平台二进制、SHA256SUMS、GitHub attestation、SBOM、GHCR 镜像 digest
- 发布自动化在首个 crate 落地后启用；当前先保留规范与占位 CI

## 待定事项

- v2 路线优先级
- 实际 MSRV（候选 Rust 1.82，待运行时依赖定稿后校准）
- crates.io 包名保留与首个发布凭据配置
- 独立治理邮箱（当前 Code of Conduct 走 GitHub 私密报告）
- 自定义域名（当前使用 GitHub Pages）

---

## Agent skills

### Issue tracker
GitHub Issues (`geeknonerd/stuntdouble`). Use `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels
Canonical roles mapped to exact label strings. See `docs/agents/triage-labels.md`.

### Domain docs
Single-context repo; ADRs in `plans/adr/`; glossary `CONTEXT.md` (created lazily). See `docs/agents/domain.md`.

--- 

*注：本文档由项目调研文档自动化汇总生成；实现代码出现后，请以代码中的注释与类型注释为准补充实现细节。*

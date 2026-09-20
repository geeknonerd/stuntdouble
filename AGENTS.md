# AGENTS.md

本文件面向在本仓库工作的编码代理。它只保留代理必须知道的约束与入口，详细规则以链接文档为准。

## 先读

1. [CONTEXT.md](CONTEXT.md) — 领域词汇，输出命名以此为准。
2. [README.md](README.md) — 项目目标与当前状态。
3. [plans/product-definition.md](plans/product-definition.md) — v1 范围与不做清单。
4. [docs/contracts/](docs/contracts/) — 配置、`ctx` API、CLI 公开契约。
5. [docs/development.md](docs/development.md) — Git、提交、CI、版本号、发布规则。
6. [plans/adr/](plans/adr/) — 架构决策，先检查是否与现有 ADR 冲突。
7. [docs/agents/domain.md](docs/agents/domain.md) — 文档消费规则与仓库布局。

## 项目快照

Stunt Double 是 Rust 实现的 Mock Server，面向对接真实外部依赖的集成联调。第一版（T1）已落地 `serve`/`validate` 子命令与 TOML 配置加载、路由匹配、HTTP 404/501 响应以及结构化日志；脚本运行时和上游数据源待后续切片完善。
仓库采用 bin+lib 结构：src/lib.rs 暴露公共模块（config/matcher/server），src/main.rs 作为 CLI entry point。详见 [docs/solutions/ci/doctest-lib-target-required.md](docs/solutions/ci/doctest-lib-target-required.md)）。
- 核心能力：外部数据源、脚本变换、文件与二进制响应、内置 JS/Python、零外部运行时依赖。
- 所有接口只有一条执行模型：`match → source → transform → response`。
- 详细范围见 [plans/product-definition.md](plans/product-definition.md)。

## 仓库布局

```text
/
├── AGENTS.md              # 本文件
├── CONTRIBUTING.md        # 贡献指南
├── ... (see REAMDE.md for full list)
├── src/
│   ├── lib.rs            # 公共模块：config, matcher, server
│   └── main.rs           # CLI entry point
├── tests/                # e2e & unit tests
├── docs/
│   ├── README.md                     # 文档索引
│   ├── development.md                # 开发与发布流程
│   ├── contracts/                    # 公开契约（配置、ctx API、CLI）
│   └── agents/                       # issue tracker、labels、domain 文档
│   └── solutions/                    # 已解决问题的学习记录（ce-compound）
├── plans/
│   ├── product-definition.md
│   ├── demo-document-catalog.md
│   └── adr/                          # ADR 0001–0012
└── research/                         # 调研与选型证据
```

## 不可破坏约束

- 不新增第二套执行路径。任何能力都必须落在 `match → source → transform → response`。
- 脚本能力只经宿主注入的 `ctx` API 提供；不暴露裸 `fetch`、`fs`、`os`、`subprocess`、`socket`。
- 静态文件目录是唯一文件根；拒绝绝对路径与 `..`。
- v1 不提供请求间共享状态，不得把状态藏进脚本闭包或模块变量。
- 上游有 HTTP 响应即视为数据，默认透传；传输层失败才抛异常。
- 安全修复不得保留给付费层。
- 新增依赖必须符合 `MIT OR Apache-2.0`，并说明标准库或已有依赖为何不够。
- 配置格式固定为 TOML；契约变更按 [docs/development.md](docs/development.md) 的弃用规则处理。

## 开发流程

- `main` 始终可发布；所有改动走短分支 + PR。
- 分支命名：`feat/`、`fix/`、`docs/`、`chore/`、`ci/`、`release/`。
- 只允许 squash merge；PR 标题使用英文 Conventional Commits。
- 每个 commit 必须 `git commit -s`（DCO）。
- CI 硬门槛：`fmt`、`clippy`、`test`、`docs`、`deny`、`audit`、`msrv`、`pr-title`、`dco`。
- 版本号遵循 SemVer，tag 为 `vMAJOR.MINOR.PATCH`。
- 发布由 `release-plz` + `cargo-dist` 驱动；详细规则见 [docs/development.md](docs/development.md)。
- 不要直接 push、force push 或删除 `main`。

## 本地检查

代码落地后，提交前运行：

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo doc --no-deps
cargo deny check
cargo audit
```

当前仓库没有 `Cargo.toml`。改文档时至少验证本地 Markdown 链接与 YAML 语法。

## 安全与隐私

- 公开 issue、PR、日志、fixture、截图不得包含客户数据、内网地址、Token、生产日志、请求体或响应体。
- 示例统一使用 `plans/demo-document-catalog.md` 中的公开演示名称。
- 漏洞走 GitHub 私密报告，见 [SECURITY.md](SECURITY.md)。
- 脚本沙箱、SSRF、路径穿越、上传与流式响应改动必须有回归测试。

## 文档规则

- 根目录社区文档使用英文；设计文档可以使用中文，欢迎补英文翻译。
- 公开契约变更必须同步更新 `docs/contracts/` 与 `CHANGELOG.md`。
- 领域术语变化同步更新 `CONTEXT.md`。
- 难以逆转的决策写入 `plans/adr/`，编号递增，文件名使用 kebab-case。
- 不要复制大段契约内容到本文件；链接到权威文档。

## 代码理解

- 如果仓库根目录存在 `.codegraph/`，先用 `codegraph explore "<symbols or question>"` 或 MCP `codegraph_explore`，再使用 grep/find。
- 修改共享函数前先追踪调用链，根因在共享层修复。
- 非平凡逻辑必须留下一个最小可运行检查。

## Agent skills

### Issue tracker
GitHub Issues (`geeknonerd/stuntdouble`)，使用 `gh` CLI。见 [docs/agents/issue-tracker.md](docs/agents/issue-tracker.md)。

### Triage labels
五个 canonical triage 角色映射到同名 label。见 [docs/agents/triage-labels.md](docs/agents/triage-labels.md)。

### Domain docs
单上下文仓库；ADR 在 `plans/adr/`；词汇表为 `CONTEXT.md`。见 [docs/agents/domain.md](docs/agents/domain.md)。

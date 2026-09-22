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

Stunt Double 是 Rust 实现的 Mock Server，面向对接真实外部依赖的集成联调。T1 落地 `serve`/`validate` 子命令、TOML 配置加载、路由匹配与结构化日志；T2 起命中路由会在内置 Boa 运行时执行 JavaScript，宿主注入的 `ctx`（`apiVersion`/`request`/`respond`/`log`/`env`）产生响应，脚本异常映射为 500 `script_error` 或 `script_no_response`；T4 增加 allowlist 约束的 `ctx.http.get`，上游 4xx/5xx 视为数据，未捕获的传输层失败映射为 502 `upstream_unreachable`；T5/T6 提供 `demo/` 演示夹具：清单路由经 `ctx.http.get` 生成 CSV，下载路由经 `ctx.http.pipe` 把上游 2xx 响应体流式转发给客户端（转发 Range、保留 206/`Content-Range`），并区分 `upstream_url_invalid` / `upstream_redirect_error` / `upstream_http_error` 等可捕获错误（见 ADR 0005 T6 修订）；T7 补齐每请求结构化日志（命中路由、脚本耗时、上游调用链、请求/响应体大小与白名单 header）：`http.pipe` 在 body 结束或客户端断开时定稿，中途上游读失败记为 `upstream_stream_error`；`ctx.log.*` 消息由脚本自担脱敏责任，运行期日志属运维诊断面、发布前必须脱敏；`serve --verbose` 为引擎生成的 500/502 附加稳定的 `detail` 类别，绝不向客户端泄露堆栈、脚本消息、上游 body 或内部地址；静态文件读取、文件流与上传待后续切片完善。
仓库采用 bin+lib 结构：src/lib.rs 暴露公共模块（config/matcher/script/server）；upstream 为内部模块，src/main.rs 作为 CLI entry point。详见 [docs/solutions/ci/doctest-lib-target-required.md](docs/solutions/ci/doctest-lib-target-required.md)）。
- 核心能力：外部数据源、脚本变换、文件与二进制响应、内置 JS/Python、零外部运行时依赖。
- 所有接口只有一条执行模型：`match → source → transform → response`。
- 详细范围见 [plans/product-definition.md](plans/product-definition.md)。

## 仓库布局

```text
/
├── AGENTS.md              # 本文件
├── CONTRIBUTING.md        # 贡献指南
├── ... (see README.md for full list)
├── src/
│   ├── lib.rs            # 公共模块：config, matcher, script, server；upstream 为内部模块
│   ├── main.rs           # CLI entry point
│   └── ...               # config, matcher, script, server, upstream 实现
├── tests/                # 通过构建出的二进制做端到端测试
├── demo/                 # 演示夹具：配置 + 脚本（端到端测试使用，见 demo/README.md）
├── types/
│   └── ctx-api-v1.d.ts   # apiVersion 1 类型定义（T8 发布）
├── docs/
│   ├── README.md                     # 维护者文档索引（C 层）
│   ├── index.md                      # GitHub Pages 首页（B 层，配 index.zh-CN.md）
│   ├── guide/                        # 使用指南（B 层，英文 + .zh-CN.md 译本）
│   ├── development.md                # 开发与发布流程（C 层）
│   ├── contracts/                    # 公开契约（B 层，各配 .zh-CN.md 译本）
│   ├── agents/                       # issue tracker、labels、domain 文档（C 层）
│   └── solutions/                    # 已解决问题的学习记录（ce-compound）；按类别归档，frontmatter 含 module/tags/problem_type，实现或排障前可先检索
├── plans/                            # 产品定义、演示场景与 ADR（C 层）
│   ├── product-definition.md
│   ├── demo-document-catalog.md
│   └── adr/                          # ADR 0001–0013
└── research/                         # 调研与选型证据（C 层）
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
- CI 硬门槛：`fmt`、`clippy`、`test`、`docs`、`docs-links`、`deny`、`audit`、`msrv`、`codeql`、`pr-title`、`dco`。
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

代码改动后跑上一节列出的本地门禁；只改文档时至少检查 YAML 语法，Markdown 本地链接与双语配对由 `docs-links` 门槛自动校验，可用 lychee 本地预检。

## 安全与隐私

- 公开 issue、PR、日志、fixture、截图不得包含客户数据、内网地址、Token、生产日志、请求体或响应体。
- 示例统一使用 `plans/demo-document-catalog.md` 中的公开演示名称。
- 漏洞走 GitHub 私密报告，见 [SECURITY.md](SECURITY.md)。
- 脚本沙箱、SSRF、路径穿越、上传与流式响应改动必须有回归测试。

## 文档规则

- 文档语言分三层：A 层英文单语、B 层英文 + `.zh-CN.md` 译本、C 层中文；见 [ADR 0013](plans/adr/0013-documentation-language-and-bilingual-structure.md) 与 [docs/development.md](docs/development.md)。
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

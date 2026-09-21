# Issue Tracker：GitHub

本仓库的 issue 与 PRD 都是 GitHub issue。所有操作使用 `gh` CLI。

- 仓库：`geeknonerd/stuntdouble`
- 远端：`git@github.com:geeknonerd/stuntdouble.git`

## 约定

- **创建 issue**：`gh issue create --title "..." --body "..."`。多行正文用 heredoc。
- **读取 issue**：`gh issue view <number> --comments`，用 `jq` 过滤评论，并一并读取 labels。
- **列出 issue**：`gh issue list --state open --json number,title,body,labels,comments --jq '[.[] | {number, title, body, labels: [.labels[].name], comments: [.comments[].body]}]'`，配合相应的 `--label` 与 `--state` 过滤。
- **评论 issue**：`gh issue comment <number> --body "..."`
- **添加 / 移除 label**：`gh issue edit <number> --add-label "..."` / `--remove-label "..."`
- **关闭**：`gh issue close <number> --comment "..."`

仓库信息从 `git remote -v` 推断；在克隆目录内运行 `gh` 时会自动完成。

## PR 作为 triage 入口

**PRs as a request surface: no.** _（该标志由 `/triage` 读取，保持英文原文；若本仓库把外部 PR 当作功能请求，改为 `yes`。）_

设为 `yes` 时，PR 与 issue 走同一套 label 与状态，使用对应的 `gh pr` 命令：

- **读取 PR**：`gh pr view <number> --comments`，diff 用 `gh pr diff <number>`。
- **列出待 triage 的外部 PR**：`gh pr list --state open --json number,title,body,labels,author,authorAssociation,comments`，只保留 `authorAssociation` 为 `CONTRIBUTOR`、`FIRST_TIME_CONTRIBUTOR` 或 `NONE` 的项（去掉 `OWNER`/`MEMBER`/`COLLABORATOR`）。
- **评论 / 打 label / 关闭**：`gh pr comment`、`gh pr edit --add-label`/`--remove-label`、`gh pr close`。

GitHub 的 issue 与 PR 共用一个编号空间，因此裸 `#42` 可能是两者之一：先用 `gh pr view 42` 解析，再回退到 `gh issue view 42`。

## 附加 label

canonical triage label 定义在 `docs/agents/triage-labels.md`；一个普通 issue 最多带一个 triage label。

状态 label：

- `blocked`：依赖另一个 issue、PR 或外部状态
- `security`：安全、沙箱、SSRF、路径穿越或信任边界问题

区域 label 不互斥，通常使用其中一个或多个：

- `area:config`：配置加载与校验
- `area:match`：method、path、params 与 query 匹配
- `area:source`：静态文件与上游 HTTP 数据源
- `area:transform`：JavaScript 或 Python 变换流水线
- `area:response`：状态、header、body、二进制响应、Range、流式
- `area:sandbox`：`ctx` API、宿主限制、脚本安全边界
- `area:runtime-js`：Boa JavaScript 运行时
- `area:runtime-python`：RustPython 运行时
- `area:files`：静态文件目录、上传、文件流
- `area:observability`：日志、request ID、指标、诊断
- `area:distribution`：发布、包、容器镜像、CI

在这些之外，沿用 GitHub 既有的类型与社区 label：`bug`、`enhancement`、`documentation`、`question`、`good first issue`、`help wanted`、`duplicate`、`invalid`、`accessibility`、`wontfix`。

隐私规则：issue、评论、日志、fixture 与截图必须使用 `plans/demo-document-catalog.md` 中的公开演示名称（`/demo/documents/...`、`DOC-0001`、`metadata.example.com`、`files.example.com`）。不得粘贴客户路径、主机名、header、token、生产日志、请求体或响应体。

## 当技能说「发布到 issue tracker」

创建一个 GitHub issue。

## 当技能说「取回相关 ticket」

运行 `gh issue view <number> --comments`。

## Wayfinding 操作

供 `/wayfinder` 使用。**map** 是单个 issue，**child** issue 是 ticket。

- **Map**：带 `wayfinder:map` label 的单个 issue，正文承载 Notes / Decisions-so-far / Fog。`gh issue create --label wayfinder:map`。
- **Child ticket**：以 GitHub sub-issue 形式挂到 map 上的 issue（用 `gh api` 调用 sub-issues endpoint）。未启用 sub-issue 时，把 child 加进 map 正文的任务列表，并在 child 正文顶部写 `Part of #<map>`。label 用 `wayfinder:<type>`（`research`/`prototype`/`grilling`/`task`）。被认领后，ticket 指派给推进的开发。
- **Blocking**：使用 GitHub **原生 issue dependency**，这是 canonical 且 UI 可见的表示。用 `gh api --method POST repos/<owner>/<repo>/issues/<child>/dependencies/blocked_by -F issue_id=<blocker-db-id>` 添加边，其中 `<blocker-db-id>` 是 blocker 的数值 **database id**（`gh api repos/<owner>/<repo>/issues/<n> --jq .id`，不是 `#number` 或 `node_id`）。GitHub 以 `issue_dependencies_summary.blocked_by` 报告（只含未关闭的 blocker，即实时闸门）。依赖功能不可用时，回退到在 child 正文顶部写 `Blocked by: #<n>, #<n>`。所有 blocker 关闭后 ticket 才算 unblocked。
- **Frontier 查询**：列出 map 下未关闭的 child（`gh issue list --state open`，限定在 map 的 sub-issue / 任务列表内），去掉仍有未关闭 blocker（`issue_dependencies_summary.blocked_by > 0`，或 `Blocked by` 行中仍有未关闭 issue）或已有 assignee 的项；按 map 顺序取第一个。
- **Claim**：`gh issue edit <n> --add-assignee @me` —— 会话中的第一次写操作。
- **Resolve**：`gh issue comment <n> --body "<answer>"`，然后 `gh issue close <n>`，再把上下文指针（要点 + 链接）追加到 map 的 Decisions-so-far。

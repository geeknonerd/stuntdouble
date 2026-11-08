---
title: "Script file writes are opt-in artifact output, not shared state, and their write path validation stays separate from reads"
date: 2026-11-07
last_updated: 2026-11-08
category: conventions
module: script file writes
problem_type: convention
component: server
severity: high
applies_when:
  - "Adding or changing a script capability that writes persistent data under the configured file root"
  - "Deciding whether a write-time artifact can be used as cross-request coordination state"
  - "Reviewing read/write path resolvers that must diverge on symlink policy"
  - "Locking atomic-replace guarantees (hard-link isolation, concurrent-reader visibility) with regression tests"
related_components: [files, script, tests]
tags: [ctx-file, atomic-replace, hard-link, symlink, artifact-output, fail-closed, budgeted-writes]
---

# 脚本文件写入是 artifact output，不是 Shared state，写入路径校验与读取保持分离

## Context

脚本现在可以经 `ctx.file.writeText` / `ctx.file.writeBytes` 在同一 File root 内创建或原子替换文件（见 [ADR 0022](../../../plans/adr/0022-script-file-writes.md) 与公开契约 `docs/contracts/ctx-api.md`）。设计落定前必须回答三个问题，且都是“猜错就会形成一类隐蔽缺陷”的决策：

1. **持久化语义**：写文件天然跨请求、跨重启可见。若不先限定，它很容易被当作 `ctx.state` 的廉价替代品，或者被要求实现“请求时写入、响应后删除”的临时文件语义。但当前脚本没有请求前/响应后生命周期 hook：worker 在产生最终结果后即退出，流式响应期间脚本已结束（`plans/adr/0022-script-file-writes.md` 背景段）。于是“写入后清理”无法靠现有脚本结构实现，必须是宿主级清理器，这是另一项能力。
2. **路径校验的对称性诱惑**：文件读取与写入都以 canonicalize + `starts_with(root)` 确认不越界（`src/files.rs:854` 的 `resolve_for_write` 与读路径的 `resolve`，`src/files.rs:953`），看起来可以抽公共函数。但两者在 symlink 语义上相反：
   - 读取：允许解析到 root 内的 symlink，只要求 canonical target 仍在 root 内；
   - 写入：拒绝任何 symlink 组件（含最终目标），且目标必须是普通文件。
   把对称部分抽成一个“通用 resolve”会把这条分叉藏进参数/标志，后人极易在后续修改时把写路径放宽为读路径的规则，或反过来。
3. **原子性的可证伪性**：ADR 承诺“替换断开硬链接、读者只见完整旧/新版本”，但这些保证只能靠 rename 语义成立；如果实现退化为截断写，就需要测试能立刻失败。第一版只测了“并发写同一路径最终是某个完整版本”，硬链接隔离与并发读者两项保证还没有回归锁。

## Guidance

### 1. 文件写入默认关闭，启用后声明为 artifact output

- `files.writable = false` 时，通过脚本侧参数校验的写入抛可捕获的 `file_write_disabled`，不触碰磁盘；`files.write_max_bytes` 是每请求成功写入预算，单次调用上限 8 MiB（`src/config.rs:54-56`、`src/files.rs:820-834`）。参数校验（形状、编码、8 MiB）先于启用状态检查，因此非法参数不会被伪装成配置错误；路径合法性仍由宿主在启用门禁之后判定（详见 [参数校验先于启用门禁](validate-before-enablement-and-log-bridge-crossings.md)）。
- 写入跨请求、跨重启保留，不自动清理，脚本随后抛错、超时或客户端断开不回滚；它不是 Shared state，不提供原子读改写、CAS、事务、TTL 或跨实例协调，不得替代 `ctx.state`。
- 同步公开契约、类型定义、CHANGELOG 与双语文档（`docs/contracts/ctx-api.md`、`types/ctx-api-v1.d.ts`）；领域术语同步（`CONTEXT.md` 中 `Static file root` 改为 `File root`）。

### 2. 写入路径校验独立实现，注释写明与读取的语义分叉

- 写入用 `resolve_for_write`（`src/files.rs:854`）：拒绝空路径、绝对路径、`..`、任何 symlink 组件（父目录和最终目标）；父目录必须已存在，宿主不自动创建；Windows 额外拒绝 ADS（`:`）、保留设备名、结尾点/空格与非法字符。
- 读取仍用 `resolve`：允许 root 内 symlink、只做 canonical 包含性检查。两路的 canonicalize + `starts_with` 表面重复是刻意保留的安全分叉，不要抽公共函数；若有人提出去重，只接受“加注分叉点”的注释，不接受合并校验逻辑。

### 3. 原子替换用目标目录临时文件 + rename，并用反例测试锁死保证

- 实现：`atomic_replace`（`src/files.rs:1012`）在目标目录内建临时文件、写入、`persist` 到目标；不 `fsync`；失败保留旧文件；权限使用临时文件默认 `0600`。
- 硬链接隔离：`rename` 只替换目录项指向的 inode，外部硬链接路径不受影响；原地截断写会改写同一 inode，必须由测试拦截。
- 并发读者：rename 原子切换使读者只见完整旧版或新版；Windows 的替换 API 在目标打开时可能报共享冲突，写入失败要有上层错误码而非部分文件。并发 writer 还会在 Windows 上互相抢同一目标（`MoveFileExW` 可返回 `ERROR_ACCESS_DENIED`）；宿主进程内必须串行化 replace 步骤或做有界重试，并用 `test-windows` 的多轮并发测试锁住。
- 重试与 deadline：Windows 上最多 3 次尝试（含首次，2 次退避，增量约 30ms），非 Windows 2 次（`PERSIST_ATTEMPTS` 定义见 `src/files.rs:1085`、`:1088`）；单次写入入口与每次尝试、每个退避窗口前都检查脚本 deadline，已超时直接返回 `file_io_error` 且不落盘（`src/files.rs:841`、`:1048`、`:1063-1066`）。重试只是平台补偿，不改变 ADR 0022 的 last-writer-wins 与“不提供锁/队列”语义。
- 回归测试：并发读者用例要求 4 个读线程都进入读循环后才开始写入，并各自完成至少 4 次循环读，避免 schedule 饥饿让并发断言空转（`src/files.rs:1426`、`:1434`、`:1493`、`:1507-1509`）。

## Why This Matters

若把 artifact 当作临时文件或 `ctx.state` 替代品，API 契约会承诺宿主并不提供的协调与清理语义，后续只能在用户已依赖的位置补实现。生命周期 hook 的缺失使该风险无法用脚本代码规避；能做的只有把语义边界写死在 ADR 与契约里。

读取与写入路径的对称重复是安全语义的分叉点，不是代码重复。合并它们会把“写禁 symlink”这条硬规则降级为可配置的细节，复审时看不出来，最终以越界写入形式爆雷。业界对 TOCTOU/路径校验的经验（OWASP 输入校验清单、CWE-22/CWE-367）共识同样是：同根、同 canonical 校验，但读/写语义分开。

硬链接与并发读者的保证来自 rename 的 inode 语义（POSIX `rename(2)` 只切换目录项），不是 Rust 代码的偶然性质。若测试只先行覆盖“并发写完整版本”，实现退化为截断写时仍能通过，硬链接隔离承诺会静默失效；因此要用反例测试把 rename 语义锁住。

## When to Apply

- 任何新增“脚本可持久化写文件”或“宿主代脚本落盘”的能力。
- 引入请求级临时工作区、请求后清理器或生命周期 hook 的提案（需新 ADR，不得悄悄复用本能力）。
- 复审 `ctx.file` 的 resolve / resolve_for_write / atomic_replace，或新增同类文件根写入路径时。
- 评估平台专属路径校验（如 Windows ADS/保留名）的测试方式：当前 `#[cfg(windows)]` + CI `test-windows` 已够用；去 cfg 化的纯函数抽取可延后到下次大改动 files 模块时。

## Examples

- 写路径拒绝集（fail-closed，`file_path_invalid`）：空路径、单点、尾部分隔符路径、含父目录跳转的组合、绝对路径、`..`、任意 symlink 组件；父目录缺失与非普通文件目标为 `file_io_error`。补测时按参数化断言 `ok == false` 且 `code == file_path_invalid`（`src/files.rs` 中 `script_write_rejects_empty_and_dot_paths`）。
- 硬链接隔离测试（`#[cfg(unix)]`）：根内文件与根外文件建硬链接，写入新内容后断言根外路径仍为旧内容、根内路径为新内容。
- 并发读者测试（`#[cfg(unix)]`）：4 个读线程在 20 次重写期间循环 `readText`，每次断言等于完整旧版或完整新版。
- 预算与错误契约：单次 8 MiB、每请求 `files.write_max_bytes`、失败写入不扣预算；错误码 `file_write_disabled` / `file_path_invalid` / `file_too_large` / `file_encoding_error` / `file_io_error`（`src/files.rs:820-834`、`src/files.rs:84-92`）。

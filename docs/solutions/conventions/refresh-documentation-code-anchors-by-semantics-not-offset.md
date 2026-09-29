---
title: "Refresh documentation code anchors by intended semantics, not line offsets"
date: 2026-09-29
category: conventions
module: documentation code-anchor maintenance
problem_type: convention
component: documentation
severity: medium
root_cause: missing_workflow_step
resolution_type: documentation_update
applies_when:
  - "当重构改变源码行号，或替换了 Markdown 中 `path:line` / `path:start-end` 锚点指向的实现时"
  - "当旧锚点在本轮 diff 之前已经漂移，不能把统一行号偏移直接套用到所有引用时"
  - "当正文描述的是某个符号、语句、协议消息或测试，而当前行号只提供候选位置时"
  - "刷新 learning 文档中的代码引用后，需要用预期 token 断言、相对链接检查和 YAML 解析验证结果时"
  - "清理已被后续合并事实推翻的 merge-state 句子（例如分支尚未合并）时"
related_components: [documentation, script, tests]
tags: [documentation-maintenance, code-anchors, line-references, semantic-verification, range-token-checks, refactor, learning-docs, docs-drift]
---

# 按意图语义刷新文档中的代码锚点，而不是按行号偏移

## Context

PR #104 将 `src/script.rs` 的 worker 通信收紧为类型化协议：`BridgeName`、`ParentMessage` 与 `WorkerMessage` 在当前树中明确描述可调用的宿主桥、父到子消息和子到父消息（`src/script.rs:598-639`）。这次重构没有改变公开 CLI、`ctx` API 或配置契约，却使两篇既有 knowledge-track 文档中的几十个 `src/script.rs` 行号引用发生移动，例如 `docs/solutions/conventions/script-worker-contract-max-and-post-deadline-held-slot-tests.md` 与 `docs/solutions/architecture-patterns/blocking-upstream-body-to-async-stream-bridge.md`。

真正的问题不只是“行号变了”。部分旧锚点在这次重构之前已经错位，因此用 diff 计算一个统一偏移量再批量套到旧锚点上，会把错误引用从一个位置搬到另一个位置，并继续显得可信。已确认的代表性例子是：旧锚点 `src/script.rs:1288-1294` 被用来证明 `unexpected_worker_error` 会将协议失败映射到 `Error::WorkerTerminated`，但它实际落在 `forward_worker_stderr`；语义正确的当前范围是 `src/script.rs:1454-1460`。同样，旧锚点 `1220`、`1249`、`1257` 对应三个 deadline 站点，却分别落在函数参数或 JSON 解析附近；当前正确位置是 stdout 读取 `src/script.rs:1397`、宿主分发 `src/script.rs:1416` 和 stdin 回写 `src/script.rs:1424`。

因此，本次刷新采用的规则不是“按代码位移修补数字”，而是“按文档命题重新定位代码”：先说明一个锚点究竟要证明什么，再在当前树中找到承载该命题的符号或语句，最后用该范围内的具体 token 做断言。`src/script.rs:662-668` 到 `src/script.rs:832-838` 的 `ctx.http.pipe` 桥接锚点、以及 error class/status/detail 各自收窄到 `src/script.rs:188-201`、`src/script.rs:203-215`、`src/script.rs:220-223`，都是这条规则的实例。

这一模式并非孤例：session history 显示，2026-09-23 的一次窄范围 refresh 也遇到旧引用区域被整体替换——`docs/solutions/conventions/serve-shutdown-second-signal-semantics.md` 中旧的 `src/server.rs:171-188` 指向的接线后来被 `serve_connections` 路径取代——当时同样选择按语义重新定位，而不是平移行号。（session history）

## Guidance

1. **Inventory anchors。** 先从所有相关文档中提取 `src/script.rs:<line>` 或 `src/script.rs:<start>-<end>`，并记录锚点所属的整句话或段落。不要只收集数字；同一个函数可能在多个命题下出现，而每段 prose 需要的证据范围并不相同。

2. **Resolve intended semantics。** 对每个锚点写出一个可检验的命题。例如“stdout 读取受共享 deadline 约束”“`ctx.http.pipe` 的原生 worker bridge 是 `worker_http_pipe`”“超时 detail 精确为 `script exceeded the configured timeout`”“父进程在 `HostState` 中持有 pipe body”。旧行号只是查找线索，不是当前事实。

3. **Locate current code。** 用稳定符号名、调用形式或稳定字符串在当前树中定位语义承载点，例如 `unexpected_worker_error`、`worker_http_pipe`、`BridgeName::HttpPipe`、`timeout_at(deadline, ...)`、`self.pipe = Some(response.body)`。优先找到定义、分支或调用语句本身，而不是相近的函数。

4. **Re-anchor。** 为命题选择最小但自足的范围。单行证据就保留单行；需要同时展示枚举定义和返回分支时再扩到连续块。多个独立站点必须保留为多个锚点，不能用一个宽范围掩盖缺失的证据。例如三个 deadline 站点分别锚定为 `src/script.rs:1397`、`src/script.rs:1416`、`src/script.rs:1424`，而不是重新合并成一个大致区间。

5. **Token-assert。** 为每个新范围指定必须出现的关键 token，并让检查失败时指出具体文件和范围。此次刷新对 `src/script.rs` 的每个重定位范围都执行了 anchor-range/token 断言；范围正确但 token 不在其中，仍应视为失败。断言应固定语义证据，而不是固定整个历史代码块，这样下一次无关重构不会制造无意义的维护噪声。

6. **Sweep adjacent prose。** 在重锚时同时检查同一句话中的合并状态、分支状态、文件路径和相对链接。行号维护会暴露已经过期的非数字断言；只更新数字会留下同一段落中的第二种错误。

## Why This Matters

行号引用是文档中的证据指针，不是装饰性链接。若 `Error::WorkerTerminated` 的说明指向 `forward_worker_stderr`，读者会从代码中得到与 prose 相反的因果解释；若三个有效期站点中的某个锚点落到 JSON 解析，读者无法确认整个文档所说的 deadline 覆盖范围。引用虽然仍是 `src/script.rs`，结论却已经失去依据。

统一的 diff offset 还依赖一个不成立的假设：所有旧锚点在变更前都正确。只要基线中已经有一个错误锚点，机械位移就会稳定地保留错误，并让它更接近其他已经正确的新锚点，从而降低人工审查的怀疑。语义重锚会迫使维护者先识别断言对象，因此已有错误会被修正，而不是被刷新掩盖。

Token assertion 把“看起来在附近”升级为可重复验证的条件：当前范围必须真的包含定义、分支或调用 token。这样以后即使再次大范围重构，维护者也可以从同样的语义 token 重新定位，而不必依赖某次 diff 的净位移。

## When to Apply

- 代码发生文件内大段移动、重命名、模块拆分，或文档引用的文件本身经历协议、枚举和函数边界调整时。
- 同一文档多次引用同一个源文件，且锚点分布在定义、调用、错误分支和测试证据等多个语义层时。
- 锚点维护看起来只需改数字，但旧引用已经多年未校验，或已知某次变更前的文档就存在错误时。
- 更新合并状态、分支状态或“尚未落地”陈述时；这些 prose 可能与行号一起过期，即使公开行为没有变化。
- 为多个互相引用的 solution 文档刷新同一批源码锚点，并希望把验证范围限制在当前命题上时。

## Examples

| 文档要证明的命题 | 记录中的旧锚点 | 当前语义锚点 | 必须出现的 token |
|---|---|---|---|
| protocol error 归入 `WorkerTerminated` | `src/script.rs:1288-1294` | `src/script.rs:1454-1460` | `fn unexpected_worker_error`、`Error::WorkerTerminated` |
| stdout 读取受同一 deadline 约束 | `src/script.rs:1220` | `src/script.rs:1397` | `timeout_at(deadline, lines.next_line())` |
| 宿主分发受同一 deadline 约束 | `src/script.rs:1249` | `src/script.rs:1416` | `timeout_at(deadline, dispatch_host(host, name, payload))` |
| stdin 回写受同一 deadline 约束 | `src/script.rs:1257` | `src/script.rs:1424` | `timeout_at(deadline, write_json_line(&mut stdin, &reply))` |
| worker 侧原生桥是 `ctx.http.pipe` | `src/script.rs:662-668` | `src/script.rs:832-838` | `fn worker_http_pipe`、`BridgeName::HttpPipe` |
| 父进程持有 pipe body | 无独立旧锚点 | `src/script.rs:1269-1286` | `BridgeName::HttpPipe`、`self.pipe = Some(response.body)` |
| timeout 的 class、status、detail | 旧 class/status/detail 范围均停在真正的 timeout detail 之前 | `src/script.rs:188-201`、`src/script.rs:203-215`、`src/script.rs:220-223` | `Self::TimedOut`、`StatusCode::INTERNAL_SERVER_ERROR`、`script exceeded the configured timeout` |

机械位移的反模式应明确禁止：

```python
# 反模式：假设所有旧锚点共享同一个净位移。
refactor_offset = current_line("exchange") - recorded_line("exchange")
new_anchor = old_anchor + refactor_offset
```

一旦 `old_anchor` 在 refactor 之前就错了，`new_anchor` 只会把一个错误引用移动到新的、仍然可信的外观位置。正确做法是先由 prose 推导命题，再定位当前代码，并用范围内的 token 做断言：

```python
def assert_anchor(file, start, end, *tokens):
    text = "\n".join(Path(file).read_text().splitlines()[start - 1:end])
    missing = [token for token in tokens if token not in text]
    assert not missing, (file, start, end, missing)

assert_anchor("src/script.rs", 1454, 1460,
              "fn unexpected_worker_error", "Error::WorkerTerminated")
assert_anchor("src/script.rs", 1397, 1397,
              "timeout_at(deadline, lines.next_line())")
assert_anchor("src/script.rs", 1416, 1416,
              "timeout_at(deadline, dispatch_host(host, name, payload))")
assert_anchor("src/script.rs", 1424, 1424,
              "timeout_at(deadline, write_json_line(&mut stdin, &reply))")
assert_anchor("src/script.rs", 832, 838,
              "fn worker_http_pipe", "BridgeName::HttpPipe")
assert_anchor("src/script.rs", 1269, 1286,
              "BridgeName::HttpPipe", "self.pipe = Some(response.body)")
```

同一轮语义检查还移除了“`feat/90-process-script-worker` 尚未合并到 `main`”这句过期陈述，因为 PR #96 已把 per-request worker process 路径合入；当前树中的 `run_worker` 会创建子进程，并用同一个 deadline 约束 spawn（`src/script.rs:1464`、`src/script.rs:1489-1499`）。这说明行号刷新应把段落当作整体证据重新核对，而不是只替换冒号后的数字。

本次刷新最终对每个重定位范围执行了 anchor-range/token 断言，并检查相对链接存在、YAML frontmatter 可解析和 `git diff --check`。这些检查共同证明：新锚点不仅能解析到当前树，而且确实承载了 prose 所声称的行为。

## Related

- [slice-status-surface-sweep.md](slice-status-surface-sweep.md) — 同类文档漂移约定：状态面与当前树描述需要语义核对；本文把方法收窄到内嵌源码行号锚点。
- [blocking-upstream-body-to-async-stream-bridge.md](../architecture-patterns/blocking-upstream-body-to-async-stream-bridge.md) — 直接刷新实例；文内已注明 T6 代码行号会漂移。
- [script-worker-contract-max-and-post-deadline-held-slot-tests.md](script-worker-contract-max-and-post-deadline-held-slot-tests.md) — 直接刷新实例；protocol-error 锚点从错误位置修正到 `unexpected_worker_error`。
- [docs/development.md](../../development.md) — `docs-links` 只验证本地链接可达与双语配对，不检查内嵌代码锚点的语义漂移（`docs/development.md:175`）。
- GitHub: [issue #95](https://github.com/geeknonerd/stuntdouble/issues/95), [PR #104](https://github.com/geeknonerd/stuntdouble/pull/104) — 触发本次锚点刷新的类型化 worker 协议变更。

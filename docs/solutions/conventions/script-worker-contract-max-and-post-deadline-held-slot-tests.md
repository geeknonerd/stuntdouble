---
title: "Test uncancellable script workers at the contract maximum and prove held slots after the reply deadline"
date: 2026-09-26
category: conventions
module: script worker capacity contract tests
problem_type: convention
component: testing_framework
severity: medium
applies_when:
  - "为脚本 worker 池的槽位上限、fast-fail 或 permit 生命周期增加或评审回归测试时"
  - "测试不可取消的 worker：客户端 reply deadline 已到，但 worker 尚未真正结束时"
  - "从已发布的契约上限推导饱和请求数，而不是复制生产实现的 available_parallelism().clamp(4, 16) 公式时"
  - "在全部契约可见槽位被占用时验证 HTTP liveness，或在 deadline 后证明槽位仍被持有时"
related_components: [server, script, tests]
tags: [contract-tests, script-worker-pool, semaphore, capacity-fast-fail, permit-lifetime, deadline-boundary, end-to-end-tests, request-id]
---

# 契约上限处测试不可取消的脚本 worker，并证明答复期限后槽位仍被持有

## 背景

PR #82 为脚本执行引入了有上限、且工作不可取消的 worker pool：`AppState` 持有 semaphore，槽位数由 `available_parallelism()` 推导并 clamp 到 4–16（`src/server.rs:57-75`）。Route 命中后，`run_script` 用 `try_acquire_owned()` 尝试取槽；取不到就立即构造 `Outcome::failed(Error::CapacityExceeded)`，不排队（`src/server.rs:261-281`）。这个错误映射为 500 `script_error`，`--verbose` 下 detail 为 `script worker capacity exhausted`（`src/script.rs:154-203`；公开契约见 `docs/contracts/ctx-api.md:79`、`docs/contracts/cli.md:26`）。

真正使测试设计变难的不是「如何制造一次容量错误」，而是 worker 的生命周期长于客户端看到的 Response。`execute` 接收 `OwnedSemaphorePermit`，把它 move 进 `spawn_blocking` 闭包；闭包用 `let _slot = worker_slot;` 持有 permit，直到 `evaluate` 返回（`src/script.rs:1144-1171`）。`sandbox.script_timeout_ms` 到达时，外层只返回 `Error::TimedOut`，不会取消已经开始的 blocking worker（`src/script.rs:1174-1181`）。因此这个 deadline 是「答复客户端」的时限，不是 worker cancellation boundary；被放弃的 worker 仍会运行，直到 loop-iteration backstop 结束它（`src/script.rs:31-39`、`plans/adr/0003-script-first-multi-runtime.md:79-87`）。

PR #82 的初版 E2E 回归覆盖了错误映射，却留下三个观测缺口：

- 测试 helper `default_script_worker_limit()` 复制了生产公式，再用 `default_script_worker_limit() + 1` 计算 probe 数；测试 oracle 因此镜像了实现。
- 它先 join 全部 runaway Response，再发 404 liveness probe；这只证明请求结算后服务仍活着，没有在槽满期间观察 HTTP。
- 它只断言批量 Response 中「出现过一次」capacity detail，没有再证明 timeout 后 permit 仍被持有。

后续修复把这些观察拆成几条独立的不变量，以公开契约而不是生产公式作为 oracle。PR #82 已 squash merge；并发 worker 问题 #29 已关闭，单 worker 的 Boa 堆硬上限仍由 #28 跟踪（`plans/adr/0003-script-first-multi-runtime.md:87`）。

## 指南

对任何「资源容量有上限、工作不可取消或不可抢占」的 blocking pool，契约级回归至少应分别证明容量 oracle、饱和期 liveness、permit 生命周期这三件事，并为每个失败 Response 断言稳定 envelope。不要让一条「看到了预期错误」的断言同时承担多种证明责任。

### 1. 用公开契约构造独立的容量 oracle

容量 probe 数量必须来自文档化的外部契约，而不是测试里复制 SUT 的公式。当前公开契约允许 4–16 个脚本 worker 槽（`docs/contracts/ctx-api.md:79`），所以测试固定：

```rust
const SCRIPT_WORKER_CONTRACT_MAX: usize = 16;
const SATURATING_SCRIPT_REQUESTS: usize = SCRIPT_WORKER_CONTRACT_MAX + 1;
```

17 个并发 runaway 请求是对公开上限的独立探测（`tests/cli.rs:1792-1793`）：若上限没有被执行，17 个请求都会开始运行，没有人收到 capacity fast-fail；若执行了任何合法的 4–16 上限，在 runaway worker 于观察窗口内不释放槽的前提下，至少一个请求会被拒绝。这里检验的是「上限不超过契约最大值」，不是精确槽数：实现可以继续按可用并行度在 4–16 内变化，测试仍成立；契约上限改变时，测试必须有意识地随契约改变，而不是随私有公式漂移。

### 2. 在饱和状态仍存在时观察 liveness

不要在 join 完所有脚本请求之后才探测非脚本 Route。用一个 channel 让第一个收到 capacity fast-fail 的线程立即通知主线程，然后在释放/join 之前发送一个不需要 worker slot 的请求（`tests/cli.rs:1797-1822`）：

```rust
let response = request(port, "GET", "/runaway", &[]);
if response.body.contains("script worker capacity exhausted") {
    let _ = capacity_tx.send(());
}
// ...
capacity_rx
    .recv_timeout(Duration::from_secs(5))
    .expect("no capacity fast-fail while the pool was saturated");
let missing = request(port, "GET", "/not-found", &[]);
```

未匹配的 Route 在 Match 阶段直接构造 `Handled::NotFound`，不会进入 `run_script`，也不依赖脚本槽（`src/server.rs:97-108`）。这一步同时验证：脚本池已经饱和、capacity 路径已经发生，而 HTTP 服务仍能在这个饱和窗口内响应；测试还断言 404 为 `not_found`（`tests/cli.rs:1913-1917`）。

### 3. 对「已放弃但仍在运行」的任务断言资源仍被持有

第一批 Response 全部 join 之后，再立即发送一个 runaway 请求，并断言它收到 capacity detail，而不是 timeout detail（`tests/cli.rs:1827-1830`、`tests/cli.rs:1918-1928`）：

```rust
let held = request(port, "GET", "/runaway", &[]);
assert!(
    held.body.contains("script worker capacity exhausted"),
    "a timed-out worker released its slot before finishing: body: {}",
    held.body
);
```

这一条是独立的 permit 生命周期证明。前面的批量请求即使只出现「某次 capacity detail」，也不足以排除「worker timeout 时错误释放 permit、同时另一个请求刚好触发拒绝」的实现。join 后的额外请求让此类错误必然表现为：它拿到刚释放的槽并运行，100 ms 后返回 `script exceeded the configured timeout`，使 held 断言失败。

### 4. 每个失败 Response 都同时断言稳定 envelope

批量 runaway Response 和 held Response 都要检查非空 `request_id`（`tests/cli.rs:1889-1897`、`tests/cli.rs:1929-1935`）。这样测试失败时既能区分容量拒绝、timeout 和其他 500，也不会让「状态码正确但 envelope 不完整」的实现以绿色通过。

## 为什么重要

- **镜像公式会让测试与缺陷一起漂移。** 旧 helper 复制 `available_parallelism().map_or(4, |p| p.get().clamp(4, 16))` 后计算 probe 数。若实现公式本身改错，测试也同步改变 probe 数，仍可能运行出「预期」的容量错误；它没有独立事实可对照。固定契约常量把 SUT 与 test oracle 分开，才能让实现回归真实失败。
- **「事后仍活着」不等于「饱和期间仍活着」。** join 后发 404 只证明最后一个 worker 答复后服务没有死；它无法发现请求处理循环在槽满时被阻塞、只等某个 worker 释放后才读取非脚本 Route 的问题。channel 把观察点钉在 capacity 信号之后、请求结算之前，覆盖的正是风险窗口。
- **一次 capacity detail 只证明拒绝路径存在，不证明资源生命周期。** 错误映射测试回答「取不到槽会怎样」，permit 测试回答「timeout 后槽是否仍被占用」。两者由不同实现分支控制，必须有不同的反事实失败模式。
- **deadline 的语义容易被误读。** `spawn_blocking` 不可取消，permit 又被送进 worker 闭包；如果把 `script_timeout_ms` 当成 worker 终止点，就会设计出只看 timeout Response 的测试，并在实现提前释放 permit 时保持绿色。测试必须按文档化生命周期观察，而不是按 API 名字猜测。
- **E2E 的证据边界也要写清。** 这套测试通过真实 HTTP 观察公开容量上限与 permit 生命周期，不直接数线程，也不测内存。ADR 0003 明确说这不是单 worker 的内存硬边界；硬内存/CPU 边界仍由 #28 的进程隔离方案跟踪（`plans/adr/0003-script-first-multi-runtime.md:83-87`）。把当前测试解释成「内存已经封顶」会制造错误安全感。

## 何时适用

- 为有界 worker pool、thread pool、semaphore 或 blocking task executor 写契约级回归时，尤其是任务开始后不能取消、deadline 只影响调用方答复的场景。
- 更新 capacity acquisition、permit ownership、timeout、fast-fail、worker shutdown 或 loop backstop 逻辑时。
- 需要验证「服务在资源饱和时仍能响应不消耗该资源的 Route/请求」时。
- 外部契约给出容量范围，但实现会根据机器并行度、运行模式或配置选择具体值时；这时应测边界性质，不测私有默认公式。
- 评审看到回归只断言「某处出现了容量错误」时；继续追问它发生在饱和窗口内，还是全部请求结算之后。
- 不适用于纯粹的错误码映射单测，也不替代内存泄漏、线程泄漏或进程级资源限制测试。

## 示例

### 容量 oracle：从镜像公式改为公开上限

PR #82 的初版测试在 PR 历史中包含这段测试侧 helper 与 probe 计算：

```rust
fn default_script_worker_limit() -> usize {
    std::thread::available_parallelism()
        .map_or(4, |parallelism| parallelism.get().clamp(4, 16))
}

let probes = default_script_worker_limit() + 1;
```

它虽然能制造饱和，却把生产公式复制进了测试。修复后的当前树改为契约常量：

```rust
const SCRIPT_WORKER_CONTRACT_MAX: usize = 16;
const SATURATING_SCRIPT_REQUESTS: usize = SCRIPT_WORKER_CONTRACT_MAX + 1;
```

如果 17 个并发 runaway 请求全部开始运行，就不会产生 capacity Response，`recv_timeout` 或 capacity 断言失败；测试因此能独立发现「上限没有执行」。

### 饱和期 liveness：从 join 后 404 改为 capacity 信号后立即 404

旧顺序是：

```rust
let responses = handles
    .into_iter()
    .map(|handle| handle.join().expect("runaway request"))
    .collect::<Vec<_>>();
let missing = request(port, "GET", "/not-found", &[]);
```

此时所有脚本请求都已经结算，404 只说明服务最终仍可用。新测试在第一个容量失败时通过 mpsc 发出信号，主线程收到信号后立即探测（`tests/cli.rs:1811-1817`）：

```rust
capacity_rx
    .recv_timeout(Duration::from_secs(5))
    .expect("no capacity fast-fail while the pool was saturated");
let missing = request(port, "GET", "/not-found", &[]);
```

若 HTTP 面在槽满时不能响应，或在处理 capacity 路径时被阻塞，这一步会失败或超时，而不会等到所有 worker 结束才暴露。

### permit 生命周期：从「出现过容量错误」改为 join 后再探测

旧测试只保留：

```rust
assert!(
    responses.iter().any(|response|
        response.body.contains("script worker capacity exhausted"))
);
```

修复提交在第一批 join 后增加 held 请求（`tests/cli.rs:1824-1830`）：

```rust
let held = request(port, "GET", "/runaway", &[]);
assert_eq!(held.status, 500);
assert_eq!(error_class(&held.body).as_deref(), Some("script_error"));
assert!(
    held.body.contains("script worker capacity exhausted"),
    "a timed-out worker released its slot before finishing: body: {}",
    held.body
);
```

这些反事实由此变得明确：

- 未执行容量上限：17 个请求全部 timeout，capacity 信号或断言失败。
- timeout 时提前释放 permit：held 请求拿到槽并最终 timeout，held 的 capacity detail 断言失败。
- 饱和时 HTTP 无法响应：信号后的立即 404 失败或超时。
- 失败 envelope 缺 `request_id`：批量或 held 的 `request_id` 断言失败。

## 相关

- [blocking-upstream-body-to-async-stream-bridge.md](../architecture-patterns/blocking-upstream-body-to-async-stream-bridge.md) — 同样记录 `spawn_blocking` worker 不可取消、reply deadline 只结束客户等待的前提；本文补齐容量上限与 permit 生命周期。
- [hold-upload-temp-dir-with-unfinished-request-body.md](../test-failures/hold-upload-temp-dir-with-unfinished-request-body.md) — 同样用契约事件持有长生命周期状态，而不是猜时间；本文的 capacity channel 是同一同步原则在 worker-pool 场景的实例。
- [serve-shutdown-second-signal-semantics.md](serve-shutdown-second-signal-semantics.md) — 同样要求先观察到状态转变再走后续分支；固定 sleep 会制造假通过。
- [request-read-deadline-at-the-connection-layer.md](request-read-deadline-at-the-connection-layer.md) — 记录「绿色回归没有跨过应保护的契约边界」的同类失败模式；本文的镜像公式与结算后 liveness 属于同一类。
- GitHub：[PR #82](https://github.com/geeknonerd/stuntdouble/pull/82)、[issue #29](https://github.com/geeknonerd/stuntdouble/issues/29)、[issue #28](https://github.com/geeknonerd/stuntdouble/issues/28)

---
title: "Test script worker capacity at the contract maximum and prove post-deadline slot reuse"
date: 2026-09-26
last_updated: 2026-09-28
category: conventions
module: script worker capacity contract tests
problem_type: convention
component: testing_framework
severity: medium
applies_when:
  - "为脚本 worker 的容量上限、fast-fail 或 deadline/permit 生命周期增加或评审回归测试时"
  - "测试按每请求进程隔离运行的 worker，deadline 到期后宿主强杀、reap 进程并释放槽位时"
  - "从公开契约上限推导饱和请求数，而不是复制生产实现的 available_parallelism().clamp(4, 16) 公式时"
  - "在槽位耗尽期间验证 HTTP liveness，或在 deadline 后证明同一槽位可被新 worker 复用时"
  - "设计覆盖 spawn、协议往返、final_result 与进程退出全生命周期的 deadline 测试时"
related_components: [server, script, tests]
tags: [contract-tests, script-worker-pool, semaphore, capacity-fast-fail, permit-lifetime, deadline-boundary, process-isolation, end-to-end-tests]
---

# 在契约上限处测试脚本 worker 容量，并证明 deadline 后槽位可复用

## 背景

issue #90 让每个命中 Route 的脚本在 fresh worker process 中运行：脚本仍拿到同一份 `ctx` 契约，父进程保留 host function 与流的所有权，子进程只运行 Boa 并通过 JSON Lines 协议与父进程往返（`plans/adr/0014-process-isolated-script-runner.md:17-20`、`src/script.rs:1187-1267`）。截至本文写作时 `feat/90-process-script-worker` 尚未合并到 `main`。

> 后续修订（#91，2026-09-28）：Linux worker 已落地 `RLIMIT_AS` 硬内存上限（默认 256 MiB、下限 64 MiB）并同步公开契约；本文其余内容仍记录 #90 的容量与 deadline 测试契约，macOS 与 Windows 仍无硬内存上限。

本文件最初的版本记录的是旧模型：`sandbox.script_timeout_ms` 只结束客户端等待，不取消不可中断的 blocking worker，超时 worker 继续持有 permit，直到循环次数兜底结束（`plans/adr/0003-script-first-multi-runtime.md:73`、`plans/adr/0003-script-first-multi-runtime.md:84`）。进程隔离使这条推理整体失效：公开契约写明脚本在 fresh process 中运行、deadline 到期时宿主立即杀死 worker 进程并释放槽位（`docs/contracts/ctx-api.md:79`）；安全说明同样把 deadline 的兑现方式记为杀死 worker 进程（`SECURITY.md:13`）；ADR 0014 的 D2/D5/D12 把「每请求一个新子进程」「到期强杀、槽位即时回收」「槽位语义是同时运行的脚本进程数上限」定为核心决策（`plans/adr/0014-process-isolated-script-runner.md:18`、`:24`、`:26`）。

因此需要保留的推理不是「超时返回哪个错误」，而是：**deadline 是 worker 资源生命周期的边界，而不只是约束客户端答复的计时器**。契约测试必须分别证明 worker 会消失、槽位会复用；「permit 仍被持有」不再是正确行为，而是泄漏探针。（文件名里的 `held-slot` 是历史命名，语义已在 #90 反转。）

设计阶段否决过的三条捷径同样值得记住（session history）：常驻 worker 池需要额外的租借、污染标记与淘汰状态机，与「请求间无共享状态」的结构保证冲突；让子进程直接持有 `ctx.http.*`、`ctx.file.*`、`ctx.request.files` 会把能力与策略边界推出主进程；为观测而新增绕过既有 `execute` 路径的入口会构成第二条可调用执行路径，违反单执行模型与 ADR 0014 D4。

## 指南

对任何「容量有上限、执行单元可被 deadline 终止」的 worker pool，契约级回归至少应分别证明容量 oracle、capacity 路径后的 HTTP liveness、进程消失、槽位复用这四件事，并为每条失败 Response 断言稳定 envelope。不要让一条断言同时承担多种证明责任。

### 1. 把同一个 deadline 覆盖到 spawn、协议交换、final_result 与进程退出

deadline 在 `execute` 中由当前时间加 timeout 一次性算出（`src/script.rs:1455-1458`），随后贯穿三段生命周期：

- spawn：`timeout_at` 包住 `Command::spawn()`，spawn 自身的阻塞也在 deadline 内（`src/script.rs:1319-1328`）；
- 运行与 host bridge：每次 stdout 读取、父进程侧 host dispatch、stdin 写回都使用同一 deadline（`src/script.rs:1220`、`src/script.rs:1249`、`src/script.rs:1257`）；
- final result 与退出：即使已经读到完整 `final_result`，仍要等 child 成功退出，等待退出本身再次受同一 deadline 约束（`src/script.rs:1363-1387`）。

这条链避免一个常见漏洞：只在「等响应」时计时，却让 spawn 卡死，或让写出结果后不退出的 worker 继续占用资源。文档与测试都必须按「deadline 覆盖整个 worker lifetime」这个边界描述。

### 2. 超时分支必须 kill、reap，然后才让槽位 drop

超时时 `run_worker` 走 `WorkerReply::TimedOut` 分支：先 `terminate(&mut child)`，再等 stderr reader 结束，最后返回 `Error::TimedOut`（`src/script.rs:1353-1357`）。`terminate` 依次执行 `kill().await` 与 `wait().await`，既杀进程也回收它（`src/script.rs:1392-1395`）。

permit 是 `execute` 的参数，绑定为 `_slot`（`src/script.rs:1444-1453`），只在 `execute` 返回、作用域结束时 drop。时间线因此是：到期 → kill → wait/reap → 返回错误 → drop permit → 槽位可被下一个请求获取。不要为了等循环次数兜底而保留槽位：100,000,000 次循环上限现在只是 disposable worker 内部的 defense in depth（`src/script.rs:38-44`），不是资源回收机制。

这不只是优化。槽位限制的是同时运行的 worker 进程（`src/server.rs:63-65`、`docs/contracts/ctx-api.md:79`）；超时进程还活着却继续占槽，会让容量提前耗尽，进程级资源也没有真正回收。容量路径仍是 `try_acquire_owned()` 失败即 fast-fail、不排队（`src/server.rs:263-279`），所以释放槽位的时机会直接决定后续请求看到 timeout 还是 capacity。

### 3. 用两个独立观测证明「进程消失」与「槽位可复用」

不要用一条断言同时回答两个问题。

第一，进程消失用 bounded process-table poll。`script_deadline_kills_the_worker_process` 先确认 server 有 child，请求返回后再等 child 从进程表消失（`tests/cli.rs:1799-1826`）。helper 每 10 ms 查一次进程表、最多等 5 s，超时即失败，不使用固定 sleep（`tests/cli.rs:262-278`）。它证明的是 kill/reap 真的发生，而不是仅仅返回了 500。

第二，槽位复用要在所有 runaway 响应结算后立即再发同一 Route，断言它拿到槽位、运行到自己的 timeout，而不是收到 capacity fast-fail（`tests/cli.rs:2131-2140`、`tests/cli.rs:2234-2248`）。两条测试互为补充：

- 没做 kill/reap：process-table poll 失败；
- 杀了进程但没释放 permit：`after_deadline` 请求返回 `script worker capacity exhausted`，timeout 断言与其反向断言同时失败（`tests/cli.rs:2234-2248`）；
- 提前释放 permit 但进程还活着：槽位复用可能通过，process-table poll 仍会失败。

因此旧版「held 请求应收到 capacity detail」的断言必须删除：它要求 timeout 后仍拿不到槽位，恰好与当前公开契约相反。

### 4. 容量 oracle 继续使用公开的 4–16 上限，并保留 capacity 路径后的 liveness

容量测试不要复制生产公式。公开契约只承诺 4–16 个并发槽位（`docs/contracts/ctx-api.md:79`），测试固定 16 与 17 个 probe（`tests/cli.rs:2101-2106`）；生产公式仍是 `available_parallelism().clamp(4, 16)`（`src/server.rs:63-65`），抄进测试会让 test oracle 随实现一起漂移。

17 个并发 runaway 请求中，任意一个收到 capacity 即通过 mpsc 通知主线程；主线程收到信号后、join 全部句柄之前，用不需要脚本槽位的未命中 Route 验证 HTTP 仍在响应（`tests/cli.rs:2108-2140`）。未命中 Route 在 match 阶段直接返回 `Handled::NotFound`，不进入 `run_script`（`src/server.rs:97-107`），所以这个 404 与脚本槽位无关。注意它证明的只是「capacity 失败之后、全部请求结算之前 HTTP 仍可响应」：信号由已经拿到响应的线程发出，信号与 404 之间其他 worker 可能已经到期并释放槽位（测试内注释里「every worker slot is held」的说法强于代码实际保证的时序）；要证明「槽位确定全满时仍可响应」，需要 barrier 让 worker 保持占用直到 404 返回。全部 runaway 结算后再做 `after_deadline` 复用断言。

每条失败 Response 仍要检查完整 envelope：500、`script_error`、非空 `request_id`（`tests/cli.rs:2143-2162`）；批量响应还要分别证明 capacity fast-fail 与 timeout 两条路径都发生过（`tests/cli.rs:2216-2227`），而不是只检查 body 里是否「出现过」某个 detail。

### 5. 按失败边界选择断言，不要把 timeout、crash、protocol error 混为一谈

- **timeout**：500 `script_error`，`--verbose` detail 精确为 `script exceeded the configured timeout`（`src/script.rs:181-190`、`src/script.rs:195-204`、`src/script.rs:210-216`）。
- **pipe 后 timeout**：父进程只在成功的 `final_result` 上把 pipe/file/upload 流交给 Response（`src/script.rs:1363-1371`、`src/script.rs:1397-1405`），timeout 路径在 `finish_outcome` 之前返回。因此要同时断言 500、`script_error`、timeout detail，以及上游 partial body 不在客户端 body 中；只断言「没有 partial body」不足以证明状态码错误（`tests/cli.rs:1995-2021`）。
- **外部 kill**：500 `script_error`、精确 detail `script worker terminated unexpectedly`，随后同一 server 的其他 Route 仍返回 200，并在结束时确认 worker child 已消失（`tests/cli.rs:1927-1991`）。
- **protocol error**：写协议失败、提前 EOF、malformed JSON、未知消息类型、`final_result` 之后的 trailing stdout 都归入 `WorkerReply::Unexpected`（`src/script.rs:1202-1203`、`src/script.rs:1214-1264`），再映射到 `Error::WorkerTerminated` 与同一稳定 detail（`src/script.rs:1288-1294`）。公开 CLI 契约明确该 detail 覆盖 protocol error 与 partial output（`docs/contracts/cli.md:26`）。当前树没有专门注入协议错误的黑盒测试，不要声称已有 E2E 覆盖；新增可控 protocol fault 时应断言与外部 kill 相同的稳定 envelope/detail。
- **capacity**：500 `script_error`、detail `script worker capacity exhausted`，同样必须带 `request_id`（`src/server.rs:263-279`、`tests/cli.rs:2143-2162`）。detail 只在 `--verbose` 下出现（`docs/contracts/cli.md:26`）。

### 6. 不把进程隔离等同于跨平台内存硬限制

#90 建立的是 deadline 强杀、进程隔离、槽位回收与稳定错误映射，没有建立内存硬限；#91 随后在 Linux 为 worker 落地 `RLIMIT_AS` 硬上限（默认 256 MiB、下限 64 MiB，见 `docs/contracts/config.md:33`），macOS 与 Windows 仍没有硬内存上限（`SECURITY.md:13`、`docs/contracts/ctx-api.md:96`）。`RLIMIT_AS` 限制虚拟地址空间而非 RSS，进程会被杀死也不等于内存已被准确封顶；不要从容量与 deadline 测试推出内存结论。

## 为什么重要

- **它固定了 deadline 的真实契约。** 对客户端仍是一个 500；对宿主，deadline 还负责终止执行、回收 OS 资源、释放容量。只覆盖前者，会让「返回了正确 JSON，但 worker 进程或槽位泄漏」的实现继续通过。
- **它把三个失败模式拆开：** 没杀进程、杀了但没 reap、reap 了但槽位没释放。process-table poll、`after_deadline` 收到 timeout、`after_deadline` 不含 capacity，各自提供独立反事实，任何一项反转都会被单独抓到。
- **它防止容量契约被过时的 held 模型锁死。** #90 之前「超时 worker 仍持有 permit」是既定行为；#90 之后同一断言变成缺陷探针。继续保留旧断言，正确的强杀实现反而会被判为失败。
- **它让容量测试保持公开契约驱动。** 4–16 上限是文档化的外部 oracle；复制私有公式会把实现错误带进测试，上下一起漂移。capacity 路径后的 mpsc + 404 liveness 把可用性观察钉在 capacity 失败之后、全部请求结算之前，而不是全部结算之后。
- **它防止「进程隔离」被过度解读。** deadline 强杀与 Linux `RLIMIT_AS` 是两个独立边界，macOS/Windows 仍没有硬内存上限。混写这些边界会制造错误安全感，并误导后续评审降低内存风险的优先级。

## 何时适用

- 修改 script worker 的 spawn、JSON Lines 交换、final-result 等待、timeout kill、child reap、permit ownership 或 capacity fast-fail 时。
- 为 fresh-process / worker-per-request 模型增加 deadline、崩溃、协议错误、pipe/file/upload 流丢弃或槽位生命周期回归时。
- 评审看到「timeout 返回 500」就认为生命周期正确时；继续追问 child 是否消失、槽位是否复用、失败响应是否仍有稳定 envelope。
- 验证公开容量范围时：probe 数从 4–16 契约推导，而不是调用生产私有公式。
- 只适用于当前「deadline 能真正终止执行单元」的模型。若执行单元重新变成不可取消，旧模型中的 held-permit 语义可能重新成立，不能机械套用本文的 `after_deadline` 断言；两者切换时必须同步刷新本文件。本文件不替代 Linux `RLIMIT_AS` 验证、RSS 观测、macOS/Windows 内存边界或整体 DoS 测试。

## 示例

### 进程确实消失：先看见 child，再从 bounded poll 等到它消失

```rust
wait_for_child_count(parent_pid, true);
let response = pending.join().expect("request thread");
assert_eq!(response.status, 500, "body: {}", response.body);
assert_eq!(
    error_class(&response.body).as_deref(),
    Some("script_error"),
    "body: {}",
    response.body
);
wait_for_child_count(parent_pid, false);
```

这是当前 `script_deadline_kills_the_worker_process` 的真实结构（`tests/cli.rs:1799-1826`）；`wait_for_child_count` 用有截止时间的轮询而不是固定 sleep（`tests/cli.rs:262-278`）。

### 槽位在 deadline 后可复用：同一 Route 应运行到自己的 timeout

```rust
let after_deadline = request(port, "GET", "/runaway", &[]);
assert_script_error_response(&after_deadline, &stderr);
assert!(
    after_deadline
        .body
        .contains("script exceeded the configured timeout"),
    "a timed-out worker kept its slot: body: {} stderr: {stderr}",
    after_deadline.body
);
assert!(
    !after_deadline
        .body
        .contains("script worker capacity exhausted"),
    "the slot was not released after the deadline: body: {} stderr: {stderr}",
    after_deadline.body
);
```

这正好替代旧版 `held` 断言：现在预期是 timeout 而不是 capacity（`tests/cli.rs:2234-2248`）。`assert_script_error_response` 同时锁定 500、`script_error` 和非空 `request_id`（`tests/cli.rs:2143-2162`）。

### 容量 oracle 与 capacity 路径后的 liveness

```rust
const SCRIPT_WORKER_CONTRACT_MAX: usize = 16;
const SATURATING_SCRIPT_REQUESTS: usize = SCRIPT_WORKER_CONTRACT_MAX + 1;

capacity_rx
    .recv_timeout(Duration::from_secs(5))
    .expect("no capacity fast-fail while the pool was saturated");
let missing = request(port, "GET", "/not-found", &[]);
```

常量来自公开的 4–16 范围（`tests/cli.rs:2101-2106`）；容量信号先到达，随后才在 join 句柄之前发送无需槽位的 404（`tests/cli.rs:2124-2140`）。它证明 capacity 路径发生后 HTTP 仍可用，不保证 404 发出时全部槽位仍被占用。不要用 `available_parallelism().clamp(4, 16)` 计算 probe（`src/server.rs:63-65`）。

### pipe 后 timeout：500 与丢弃 partial body 都要断言

```rust
assert_eq!(response.status, 500, "body: {}", response.body);
assert_eq!(
    error_class(&response.body).as_deref(),
    Some("script_error"),
    "body: {}",
    response.body
);
assert_eq!(
    json_string(&response.body, "detail").as_deref(),
    Some("script exceeded the configured timeout"),
    "body: {}",
    response.body
);
assert!(!response.body.contains("partial-body"));
```

这组断言把「没有 partial 200」拆成可证伪的条件：状态必须是 500，error envelope 必须是 timeout，上游 partial 字节不能进入客户端 body（`tests/cli.rs:1995-2021`）。父进程只在成功的 final result 之后转移流（`src/script.rs:1363-1371`、`src/script.rs:1397-1405`）。

### 外部 kill：稳定 detail 与被隔离的 Route

```rust
assert_eq!(
    json_string(&response.body, "detail").as_deref(),
    Some("script worker terminated unexpectedly"),
    "body: {}",
    response.body
);
let healthy = request(port, "GET", "/healthy", &[]);
assert_eq!(healthy.status, 200, "body: {}", healthy.body);
wait_for_child_count(parent_pid, false);
```

测试先通过进程表拿到 worker PID，再用 `kill -9` 终止它（`tests/cli.rs:1966-1972`），随后断言 crash envelope、其他 Route 的 liveness 和 child 消失（`tests/cli.rs:1973-1991`）。协议错误在实现里走同一 `WorkerTerminated` 映射（`src/script.rs:1288-1294`），但当前没有独立注入 protocol error 的 E2E。

## 相关

- [blocking-upstream-body-to-async-stream-bridge.md](../architecture-patterns/blocking-upstream-body-to-async-stream-bridge.md) — 流式与背压部分仍然有效；其 worker permit 生命周期与源码/测试行号锚点已按 #90/#91 刷新。
- [macos-rlimit-as-cannot-enforce-useful-memory-bound.md](../architecture-patterns/macos-rlimit-as-cannot-enforce-useful-memory-bound.md) — 同一进程隔离计划；RLIMIT_AS 结论不受影响，文档已用 #91 addendum 标注 #90 历史快照。
- [linux-rlimit-as-budget-and-oom-attribution.md](../architecture-patterns/linux-rlimit-as-budget-and-oom-attribution.md) — 同一 script worker 资源边界的 Linux 内存预算与 OOM 归因；与本文的容量/deadline 测试互补。
- [hold-upload-temp-dir-with-unfinished-request-body.md](../test-failures/hold-upload-temp-dir-with-unfinished-request-body.md) — 同样用契约事件持有长生命周期状态，而不是猜时间；本文的 capacity channel 是同一同步原则在 worker pool 场景的实例。
- [request-read-deadline-at-the-connection-layer.md](request-read-deadline-at-the-connection-layer.md) — 记录「绿色回归没有跨过应保护的契约边界」的同类失败模式；本文的镜像公式与结算后 liveness 属于同一类。
- [serve-shutdown-second-signal-semantics.md](serve-shutdown-second-signal-semantics.md) — 同样要求先观察到状态转变再走后续分支；worker 孤儿与关闭收尾另见 issue #92。
- [script-owned-upstream-error-mapping.md](script-owned-upstream-error-mapping.md) — 错误分类边界：worker 死亡与协议错误属于 `script_error` + `script worker terminated unexpectedly`，不进入 upstream 错误码。
- GitHub：[issue #90](https://github.com/geeknonerd/stuntdouble/issues/90)、[issue #88](https://github.com/geeknonerd/stuntdouble/issues/88)、[issue #28](https://github.com/geeknonerd/stuntdouble/issues/28)、[issue #95](https://github.com/geeknonerd/stuntdouble/issues/95)、[issue #92](https://github.com/geeknonerd/stuntdouble/issues/92)、[PR #82](https://github.com/geeknonerd/stuntdouble/pull/82)、[issue #29](https://github.com/geeknonerd/stuntdouble/issues/29)

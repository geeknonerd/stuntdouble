# Shared state worker IPC spike（#175）

- 状态：已完成（一次性原型，不进入生产代码）
- 日期：2026-10-22
- 环境：Linux x86_64、Rust 1.98.1
- 关联：[ADR 0021](../../../plans/adr/0021-v2-shared-state.md)、[Post-v1 路线](../../../plans/post-v1-roadmap.md)、[正式实现 #176](https://github.com/geeknonerd/stuntdouble/issues/176)

## 结论

**有条件架构层 GO：协议与进程模型可行，真实 worker 路径仍需在生产实现中通过集成测试确认。** 现有“每请求一个 worker + stdio JSON Lines + 宿主能力代调用”的模型可以承载进程内 Shared state，不需要第二套执行路径。

因此本 spike 只把 `host_call` / `host_result` 视为 wire-compatible 证据，不把“真实生产 worker 路径”标为已完成；正式实现与真实 `src/script.rs` 的 `HostState` / `exchange` 路径集成测试由 [#176](https://github.com/geeknonerd/stuntdouble/issues/176) 跟踪。

- `get`、`set`、`take`、`setIfAbsent` 都经 `host_call` / `host_result` 往返。
- 宿主用一个 `Mutex<Store>` 包住“检查 + 修改”，两个并发 worker 对同一 key 执行 `take` 或 `setIfAbsent` 时都恰好只有一个赢家。
- `Instant` 满足单调时钟要求；TTL 在 `now >= expires_at` 时过期，读不续期。
- 条目与字节容量都 fail closed，测试确认失败操作不淘汰已有有效项。
- 新建宿主实例后状态为空；worker 的 JavaScript globals 与 closures 不跨请求保留。
- 写入先于最终响应提交；脚本抛错或 worker 超时不会回滚已经完成的写入。

## 原型边界

这是一个独立 crate，不修改仓库根 `Cargo.toml`、`src/` 或公开 CLI。worker 由同一 spike 二进制启动，协议字段复用现有 worker IPC 的 `job`、`host_call`、`host_result`、`final_result` 形状。Boa 只负责执行脚本和把 `ctx.state` 调用转发给宿主。

worker 是针对本 spike 的聚焦实现，不是对 `src/script.rs` 的直接 patch。因此本原型证明的是现有协议与进程模型足以承载状态桥；正式接入由 #176 负责，仍需在真实 worker / `HostState` 路径补集成测试，覆盖异步 exchange、现有超时分支与 worker 回收。

worker 在显式 8 MiB 栈的专用线程上运行 Boa，与生产 worker 一致；Windows 主线程默认只有 1 MiB，直接在主线程初始化 Boa 会栈溢出。

运行：

```bash
cd research/spikes/175-shared-state-ipc
cargo test --all-targets --locked
cargo fmt -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo deny check
```

## 推荐的 wire shape

初始 job 与现有协议一致：

```json
{"type":"job","script":"...","request":{}}
```

worker 到宿主：

```json
{"type":"host_call","name":"__sd_state","payload":{"op":"get","key":"oauth:code:abc"}}
{"type":"host_call","name":"__sd_state","payload":{"op":"set","key":"oauth:code:abc","value":{"sub":"demo"},"ttl_ms":60000}}
{"type":"host_call","name":"__sd_state","payload":{"op":"take","key":"oauth:code:abc"}}
{"type":"host_call","name":"__sd_state","payload":{"op":"setIfAbsent","key":"request:abc","value":true,"ttl_ms":600000}}
```

宿主到 worker：

```json
{"type":"host_result","result":{"ok":true,"found":true,"value":{"sub":"demo"}}}
{"type":"host_result","result":{"ok":true,"found":false}}
{"type":"host_result","result":{"ok":true}}
{"type":"host_result","result":{"ok":true,"stored":true}}
{"type":"host_result","result":{"ok":false,"code":"state_invalid_input","message":"state_invalid_input"}}
```

建议保留 `found` 与 `stored` 两个独立布尔字段。`null`、`false`、`0`、空字符串和空数组都是合法状态值，不能用 `value == null` 或布尔返回值同时表示“存在”和“值本身”。

`ttl_ms` 只出现在写操作；`get` / `take` 不携带 TTL，读取不会续期。操作名使用与公开 API 一致的 `setIfAbsent`，配置字段保持 ADR 约定的 `ttl_ms`。

## 验证结果

| 验收项 | 证据 |
| --- | --- |
| 四种操作经 worker IPC | 单脚本依次调用 `set`、`get`、`setIfAbsent`、`take`，并验证返回值的深拷贝语义。 |
| 并发 `take` 恰好一个赢家 | 2 个独立 worker 进程在宿主收到 `host_call` 后经 barrier 会合再执行，重复 20 轮，每轮赢家数为 1。 |
| 并发 `setIfAbsent` 恰好一个赢家 | 2 个独立 worker 进程在 `host_call` 会合后同时写入同一 absent key，重复 20 轮，每轮恰好一个 `stored: true`，最终值等于赢家写入值。 |
| TTL 单调且读不续期 | 注入 `ManualClock`：99 ms 可见、100 ms 边界不可见；60 ms 读一次后仍按原始起点过期。`ScriptedClock` 回归测试确认每个 state 操作只使用一次时间快照，不会在清理、判断与写入之间跨过 TTL 边界。真实 worker 路径验证 TTL 写入可被另一 worker 读取。 |
| 容量 fail closed | 默认 1024 项时第 1025 项返回 `state_capacity_exceeded`，前 1024 项仍可读；默认 8 MiB 字节上限和替换失败保留旧值/TTL 另有测试。 |
| 账本一致性 | 所有容量加减使用 checked 运算；删除时发现账本下溢返回 `state_internal_error`，且在报错前不删除 entry。 |
| 重启清空、worker 不保留 JS 状态 | 同一宿主的新 worker 可见宿主状态但看不到前一个 worker 的 global；新宿主实例看不到旧状态。 |
| 提交后失败不回滚 | `set` 后脚本抛错、`set` 后 worker 超时被强杀，后续 worker 都能读到已提交值。 |

完整结果为 `23 passed`：协议/失败映射与账本单元测试 5 项、`state_ipc` 10 项、`store_clock` 8 项。完整套件连续运行 3 次均通过。

本地只在 Linux x86_64 上实测。CI 现在通过独立 `--manifest-path` 在 Linux、macOS 与 Windows 上运行该 crate 的 fmt、clippy 和测试；不把它加入 workspace，以免影响 release-plz 与 cargo-dist。

## 序列化与校验

宿主只能看到 JSON，无法在 `host_call` 到达后判断原始 JavaScript 值是否已经被 `JSON.stringify` 有损转换。因此有损值必须在 worker 侧、调用 bridge 之前预检；只靠宿主校验会把 `{ value: undefined }` 静默变成 `{}`，把 `NaN` 变成 `null`。

原型在 worker 侧拒绝并返回 `state_invalid_input` 的情况：

- `undefined`、function、symbol；
- `NaN`、`Infinity`、`-0`；
- 孤立 UTF-16 surrogate；
- 循环引用、稀疏数组、越界或非规范数组索引、数组额外属性或 symbol 属性；
- `Date` 等非普通对象，以及带 accessor、隐藏属性或 symbol key 的对象；
- 嵌套深度超过 64 的值。

宿主侧继续强制 key 最大 256 UTF-8 字节、单值紧凑 JSON 最大 16 KiB、总容量 8 MiB、最多 1024 项、TTL 范围 `1..=604_800_000` ms。worker 预检不能替代宿主侧边界，因为恶意或版本不匹配的 worker 仍可能绕过 JavaScript 层。

原型在宿主和 worker 两侧都把协议行限制为 64 KiB。现有生产 worker 的 `lines()` / `read_line` 没有这一上限；正式实现应使用同等的有界读取，避免异常脚本让协议行无限增长。

## 失败映射

| 条件 | host result / 脚本可见错误 | 处理 |
| --- | --- | --- |
| 参数、序列化或 TTL 非法 | `state_invalid_input` | 可捕获；key 原值与原 TTL 不变 |
| 条目或字节容量不足 | `state_capacity_exceeded` | 可捕获；不淘汰、不部分写入 |
| 锁中毒或宿主一致性失败 | `state_internal_error` | 可捕获；后续状态操作继续返回同一错误，不能假定存储已恢复 |
| `setIfAbsent` 命中有效 key | `{"ok":true,"stored":false}` | 不是错误 |
| worker 超时或崩溃 | 现有 `script_error` / `script_error` 路径 | 已提交写入不回滚 |

错误消息不得包含 key、value、token 或请求内容。原型只回传稳定错误码。

## IPC 竞态与超时风险

- 每个 worker 严格串行：一次 `host_call` 对应一次 `host_result`，`final_result` 之后不得再有 stdout。
- `Mutex` 临界区只包含状态校验与修改，不包含 stdout 写入、I/O 或脚本执行；否则一个慢 worker 会阻塞全部状态操作。
- 提交发生在 worker 收到 `host_result` 之前。worker 超时、脚本抛错或宿主写响应失败时，操作可能已经提交且不会回滚；这是 ADR 0021 的既定语义。
- 如果 worker 在发出 `host_call` 后崩溃，宿主可能已经处理该调用。不能通过 worker 是否收到结果推断写入是否发生。
- 宿主必须在同一 deadline 下处理“读取 worker 行、执行状态操作、写回结果、等待 worker 退出”。临界区本身很短，但锁竞争或管道阻塞仍可能把请求推到超时边界。
- `host_result` 写失败后只能终止 worker；不能假装操作未执行。
- 重启宿主会清除状态并终止在途 worker；worker 侧的 globals/closures 随进程消失，不能作为状态来源。

## 对正式实现的建议

1. 复用现有 `host_call` / `host_result` 协议，新增一个 `__sd_state` bridge；不要为四种操作各开一条协议。
2. 在 worker 侧增加一次无损 JSON 预检，再在宿主侧重复执行大小、TTL 与容量校验。
3. 用 `found` / `stored` 显式区分“缺失”与合法 falsy 值。
4. 两侧使用有界协议行读取，并把 key/value 上限纳入 bridge 的公开约束测试。
5. 将“提交后失败不回滚”和“worker 崩溃可能已提交”写入正式实现的测试与日志语义，避免调用方误以为有请求级事务。

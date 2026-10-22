# v2 Shared state：进程内、TTL 与原子操作

- 状态：`已接受`
- 日期：2026-10-22
- 关联：[ADR 0001](0001-no-shared-state-in-v1.md)、[ADR 0004](0004-host-functions-only-sandbox.md)、[ADR 0011](0011-contract-compatibility.md)、[ADR 0014](0014-process-isolated-script-runner.md)、[ADR 0016](0016-v1-ctx-pending-capabilities-deferred.md)、[Post-v1 开发路线](../post-v1-roadmap.md)、[OAuth authorization mock](../oauth-authorization-mock.md)

## 背景

[ADR 0001](0001-no-shared-state-in-v1.md) 把请求间共享状态排除在 v1 之外，并要求状态必须由宿主正式提供，不能藏进脚本闭包或 worker 变量。OAuth authorization mock 首次给出硬需求：授权码只能兑换一次，access token 需要在后续请求中校验，签名 request ID 在 10 分钟内必须拒绝重放。

脚本 worker 每请求新建并在请求结束后销毁，无法承载这些事实；若允许 worker 保留状态，会破坏隔离、超时清理与重启语义。v2 因此需要进程级 Shared state，但不引入持久化数据库或跨实例协调。

## 决策

### 生命周期与作用域

- Shared state 属于单个运行中的 `serve` 实例，由主进程持有；同一实例的所有 Route 脚本共享一个命名空间。
- 状态只存在内存中。启动时为空，正常关闭、崩溃或重启后全部清空。
- 不写磁盘、环境变量、SQLite 或上传目录；不跨端口、进程、容器实例或主机协调。
- 不提供 Admin API、配置导入导出或持久化恢复。
- 状态写入在宿主执行操作时立即生效。脚本后续抛错、客户端断开或请求超时不会回滚已完成的写入；不提供请求级事务。

### API

在 `ctx.state` 下提供四个方法：

```ts
type SdStateValue =
  | null
  | boolean
  | number
  | string
  | readonly SdStateValue[]
  | { readonly [key: string]: SdStateValue };

interface SdStateOptions {
  ttl_ms?: number;
}

interface SdState {
  get(key: string): SdStateValue | undefined;
  set(key: string, value: SdStateValue, opts?: SdStateOptions): void;
  take(key: string): SdStateValue | undefined;
  setIfAbsent(
    key: string,
    value: SdStateValue,
    opts?: SdStateOptions,
  ): boolean;
}
```

语义如下：

- `get` 返回当前值的深拷贝；状态不存在或已过期时返回 `undefined`。读取不刷新 TTL。
- `set` 原子插入或替换 key，并重置 TTL；没有 `ttl_ms` 时不过期。
- `take` 原子读取并删除 key；状态不存在或已过期时返回 `undefined`。它是“一次性授权码兑换”的唯一推荐操作。
- `setIfAbsent` 在 key 不存在、已过期或已被删除时原子写入并返回 `true`；存在有效值时不变并返回 `false`。它是 request ID 防重放的唯一推荐操作。
- 返回值是独立拷贝。脚本修改返回的数组或对象不会改变宿主状态。
- 不提供事务、批量操作、CAS、递增、scan、delete 或订阅。

### 值与 key 边界

- key 必须是非空 UTF-8 字符串，最多 256 字节；按精确字节比较，不做大小写、Unicode 或前缀规范化。建议脚本使用稳定前缀，例如 `oauth:code:`。
- value 只能包含 JSON 数据：`null`、boolean、有限 number、string、数组和普通对象。`undefined`、function、symbol、bigint、非有限 number、循环引用、稀疏数组或无法无损 JSON 化的值抛 `state_invalid_input`。
- 单个 value 的紧凑 JSON 表示最多 16 KiB。
- 每个状态项按 `UTF-8(key).len() + compact_json(value).len()` 计入容量。
- `ttl_ms` 必须是正整数安全整数，范围为 `1..=604_800_000`（7 天）。
- TTL 使用宿主单调时钟，从操作成功提交时开始计算；不受系统时间回拨影响，也不滑动续期。

### 并发与原子性

- 四个操作由宿主的状态存储在单个互斥区内完成。`take` 与 `setIfAbsent` 的“检查 + 修改”必须是一个原子步骤。
- 互斥区不跨 `await`，不执行脚本、不做 I/O、不持有 worker 引用。
- 并发兑换同一授权码时只能有一个 `take` 得到值；并发提交同一 request ID 时只能有一个 `setIfAbsent` 返回 `true`。
- 不提供跨多个 key 的原子事务。需要多步状态变更的流程由脚本排序，并接受部分写入会在失败后保留的事实。

### 容量与清理

- 最多保留 1024 个未过期或未删除的 key。
- 全部状态项的计费字节合计最多 8 MiB。
- 过期项在每次 state 操作访问对应 key 时移除；在可能新增 key 或将 value 变大前，宿主先清理已过期项。
- 达到条目或字节上限时抛 `state_capacity_exceeded`，不改动旧状态，也不做 LRU、随机或静默淘汰。
- 替换已有 key 时允许先计算新值；若新总容量超限，旧值保持不变。条目满时替换已有 key 仍允许。
- 参数校验、JSON 序列化或容量检查失败时，key 的原值与原 TTL 均保持不变。
- 不要求后台清理线程；固定上限已经约束内存，过期项最迟在下一次容量相关操作前被清理。

### Worker 桥接

- 状态存储只在主进程。worker 通过现有 host-call IPC 发送 key、value、`ttl_ms` 与操作名，主进程校验并返回结果或错误。
- worker 不获得状态存储引用，也不能把状态保存在脚本闭包、模块变量或 worker 全局中；新 worker 天然看不到旧 worker 的 JavaScript 堆。
- IPC 消息受 value/key 上限约束。宿主不得因 IPC、worker 崩溃或脚本超时而把已提交的状态写入回滚。

### 错误与日志

- 参数非法抛可捕获的 `state_invalid_input`。
- 容量不足抛可捕获的 `state_capacity_exceeded`。
- 互斥锁中毒或宿主内部一致性失败抛可捕获的 `state_internal_error`。
- `setIfAbsent` 发现有效 key 时返回 `false`，不是错误。
- 未捕获的 state 错误映射为 500 `script_error`；不新增 HTTP 状态。
- 请求日志不得记录 key 或 value。key 可能包含 code、request ID 或 token，value 可能包含凭据；日志最多记录操作名、结果、计费字节数和当前条目数。

### 版本与边界

- 这是产品里程碑 v2 的能力，但作为 `apiVersion` 1 的增量 API 发布，依据 [ADR 0011](0011-contract-compatibility.md) 不为新增成员提升 `apiVersion`。
- `ctx.state` 是 Shared state，不是 [ADR 0016](0016-v1-ctx-pending-capabilities-deferred.md) 中的请求内 `ctx.local`。不得用 `ctx.state` 保存只在单次请求内有效的数据。
- [ADR 0001](0001-no-shared-state-in-v1.md) 对 v1 的历史约束继续有效；本 ADR 只定义 v2 的受控修订。

## 后果

**正面**：OAuth 的一次性 code、token 生命周期与 request ID 防重放可以用通用原子操作实现；进程隔离与“worker 不跨请求”保持不破；固定容量避免 mock 因状态无界增长拖垮宿主。

**负面（接受）**：重启丢失全部状态，不能用于需要崩溃恢复或多副本一致性的测试；单互斥区会把所有状态操作串行化，但在 1024 项和短临界区下足够；没有事务，脚本必须显式处理部分写入。

**约束（不可破坏）**：状态只能经宿主 API 读写；不得把状态藏在脚本闭包、worker 变量、静态文件或环境变量中；不得使用静默淘汰；不得让容量错误伪装成“key 不存在”。

## 验证义务

- 两个并发 token 请求兑换同一 code：恰好一个 200，另一个 `invalid_grant`。
- 两个并发请求复用同一 `X-Sig-Request-Id`：恰好一个通过，另一个 `invalid_signature`。
- `get`、`set`、`take`、`setIfAbsent` 的缺失、过期、替换、深拷贝和返回值语义。
- TTL 使用可控单调时钟测试边界：到期前可见，等于或超过 TTL 后不可见；`get` 不续期。
- 条目上限、单值上限、总字节上限、替换失败保持旧值、过期后重新获得容量。
- 进程重启后状态为空；不同 `serve` 实例互不可见。
- 脚本在 state 写入后抛错或客户端断开，已提交写入仍可见。
- 日志不含 key 或 value，worker 崩溃不影响宿主状态存储的一致性。

## Spike 验证（#175，2026-10-22）

一次性原型位于 [research/spikes/175-shared-state-ipc](../../research/spikes/175-shared-state-ipc/README.md)，不修改生产代码，也不接入公开 CLI。结论为**有条件架构层 GO**：现有每请求 worker + stdio JSON Lines 模型足以承载本 ADR 的进程内状态，不需要第二套执行路径。原型是 wire-compatible harness，不是对生产 worker 的直接 patch；真实 `src/script.rs` / `HostState` / `exchange` 路径的集成验证与正式实现由 [#176](https://github.com/geeknonerd/stuntdouble/issues/176) 跟踪。

验证已覆盖：

- 四种操作都经 `host_call` / `host_result` 往返；两个并发 worker 对同一 key 执行 `take` 或 `setIfAbsent` 时恰好一个成功，各重复 20 轮。
- 可控单调时钟下，TTL 到期前可见、精确边界不可见，`get` 不续期；每个 state 操作只取一次时间快照，`set` 重置 TTL，过期 key 对 `setIfAbsent` 视为 absent。
- 默认 1024 项与 8 MiB 字节上限 fail closed；第 1025 项或超限写入不淘汰已有项，替换失败保留旧值与旧 TTL。
- 容量账本使用 checked 加减；删除时发现下溢返回 `state_internal_error`，不会静默截断或伪装成 key 不存在。
- 新宿主实例状态为空；新 worker 看不到前一个 worker 的 JavaScript globals/closures；脚本抛错或 worker 超时后已提交写入仍可见。

实现前明确的桥接决定：

- 复用现有 `host_call`，新增单一 `__sd_state` bridge；payload 为 `{op, key, value?, ttl_ms?}`，结果用 `found` 区分缺失与合法 falsy 值，`setIfAbsent` 用 `stored` 返回布尔结果。
- 宿主只收到 JSON，无法识别已被 `JSON.stringify` 丢失的 `undefined`、function、`NaN`、稀疏数组等值。正式实现必须在 worker 侧预检无损 JSON，并在宿主侧重复执行 key/value/TTL/容量校验。
- 两侧协议读取必须有界，避免异常 worker 让单行无限增长；状态互斥区不得包含 stdout 写入或其他 I/O。
- 写入一旦由宿主提交，worker 超时、脚本抛错或 `host_result` 写失败都不回滚；日志与错误消息不得包含 key、value 或请求内容。

## 替代方案

- **继续禁止共享状态**：无法满足 OAuth 一次性 code、token 与防重放，拒绝。
- **SQLite 或文件持久化**：引入 schema、迁移、锁、清理与崩溃恢复，不符合当前 mock 生命周期；若未来出现跨重启需求，另写 ADR。
- **跨实例共享缓存**：需要外部服务、一致性与部署配置，违背离线单二进制目标，拒绝。
- **LRU 自动淘汰**：容量满时可能静默丢失有效 code/token，测试结果不确定，拒绝。
- **用锁、事务或 Admin API 扩展成通用数据库**：超出 Mock Server 场景，增加不可控 API 面，拒绝。
- **复用 `ctx.local`**：请求内状态在请求结束即销毁，无法满足跨请求需求，且会混淆两种生命周期，拒绝。

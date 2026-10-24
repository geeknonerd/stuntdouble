---
title: "Keep script-state bridge validation authoritative in the host and bind runtime inputs structurally"
date: 2026-10-23
category: conventions
module: script worker state bridge
problem_type: convention
component: server
severity: high
applies_when:
  - "Adding or changing a JavaScript worker-to-host capability that validates structured input or mutates host-owned state"
  - "Injecting request, environment, upload, or other runtime data into a Boa realm"
  - "Choosing where to enforce size, depth, TTL, capacity, or serialization limits across worker IPC"
  - "Writing adversarial tests for replaced JavaScript globals, placeholder-shaped data, or encoder disagreement"
related_components: [script, state, tests]
root_cause: missing_validation
resolution_type: code_fix
tags: [ctx-state, worker-bridge, trust-boundary, primordials, structured-injection, host-authoritative-validation, compact-json, end-to-end-tests]
---

# 脚本状态桥以宿主为校验权威，并以结构化绑定注入运行时数据

## Context

针对 issue #176 的进程内 `ctx.state` 工作，在真实的 `HostState` / worker IPC 路径上最初沿用了两项看似合理的捷径：把 request、env 与内部标记序列化成脚本字面量后，通过 prelude 占位符替换注入；在 worker 中用脚本可见的 `JSON.stringify` 计算 value 的 compact JSON 字节数。实现后的只读审查与对抗性测试表明，这两项捷径都不能作为信任边界（session history）：

- 只调整 `.replace()` 的顺序仍不安全。先注入 `__SD_REQUEST__` 或 `__SD_ENV__` 后，后续对内部常量的替换仍可能改写刚写入的数据；根因是运行时数据进入了代码生成通道，而不是替换顺序。
- worker 的 `JSON.stringify` 可以被脚本替换，无法证明校验路径仍执行原始内建；同时 ECMAScript 与 `serde_json` 对同一个 number 可能产生不同词法，用前者计量 16 KiB 上限会拒绝宿主本来可接受的值。
- 只统计并发响应 body 中出现了几个 `"won"` 也不足以证明原子性。失败响应可能携带相同字符串，或赢家返回成功但宿主实际保存了输家的值。

当前公开约束仍由 [ADR 0021](../../../plans/adr/0021-v2-shared-state.md) 与 [`ctx` API 契约](../../contracts/ctx-api.md)定义；安全模型与边界说明见 [`SECURITY.md`](../../../SECURITY.md)。

## Guidance

### 1. 固定模板与运行时数据分离

prelude 只保留可信的编译期占位符；request、env 和内部标记必须先变成 Boa 值，再注册为临时全局。用户脚本运行前，prelude 消费这些全局并删除，之后 `ctx` 持有冻结视图。

当前实现先在 Rust 侧用 `JsValue::from_json` 转换数据，并通过 `register_global_property` 注册 `__sd_request`、`__sd_env` 与 `__sd_upstream_marker`（`src/script.rs:1235-1251`）。prelude 使用完这些值后统一执行 `delete globalThis.__sd_*`（`src/script.rs:822-830`）。不要用字符串替换注入请求体、header、环境变量或其他用户可控值；换替换顺序不是修复。

### 2. 在用户脚本运行前捕获校验所需的 primordials

凡是安全相关的 JS 校验，都不要在校验发生时重新读取可被覆盖的全局。prelude 在构造 `ctx` 时先保存所需的 JSON、Object、Array、Number、RegExp 与 Reflect 内建引用（`src/script.rs:281-299`），后续通过保存的引用调用。这样即使脚本替换 `JSON.stringify`、`Object.prototype.hasOwnProperty`、`Number.isFinite` 或 `RegExp.prototype.test`，内部校验路径也不会被关闭。

这是按用途捕获的定向加固，不是完整 SES `lockdown()`。脚本仍按半可信输入处理，不能把该模式宣传成完整沙箱。

### 3. worker 预检是诊断，宿主才是提交权威

worker 侧仍应做无损 JSON 预检，因为宿主只收到序列化后的 JSON，无法还原已经被 `undefined`、函数、`NaN` 或孤立 surrogate 丢弃的原始值。但预检不能被当作安全边界：宿主在提交前重新检查 operation、key、TTL、深度、单值与总容量，失败时不改变旧值。

单值大小必须由宿主重新编码计量，而不是使用 worker 上报的 size 或 `JSON.stringify` 的长度。当前 `Store::validate_value` 先执行宿主深度检查，再用 `serde_json::to_vec` 计算 compact bytes（`src/state.rs:180-184`）；深度上限的递归检查见 `src/state.rs:239-255`。状态提交路径在单个锁内完成存在性判断、容量计算与写入（`src/script.rs:1681-1725`、`src/state.rs:107-153`）。

### 4. 状态写入的提交点独立于脚本最终结果

`ctx.state` 的 mutation 在 host bridge 成功返回时已经提交。之后脚本抛错、超时、客户端断开或 worker 崩溃，不应回滚这次写入。把状态写入与 per-request 的 upstream/file stream 生命周期混为一谈会得到错误语义：stream 只在成功 `final_result` 后转移，而 committed state 已经属于实例级数据。

### 5. 用反事实测试锁定边界，而不是只数成功字符串

至少覆盖以下反例：

- request、env 与 request body 中含 `__SD_API_VERSION__`、`__SD_REQUEST__`、`__SD_ENV__` 等占位符形状时，它们仍按数据返回（`tests/cli.rs:1830`）。
- 替换 `JSON.stringify` / `JSON.parse` 后，合法写入仍成功，非法值仍被拒绝（`tests/cli.rs:2174`）。
- 替换属性检查、数字内建与正则方法后，稀疏数组、`NaN`、`-0` 和额外数组属性仍被拒绝（`tests/cli.rs:2222`）。
- 同一个 number 在 JS 与宿主 encoder 下词法长度不同，由宿主口径决定接受或拒绝（`tests/cli.rs:2330`）。
- 并发 `take` / `setIfAbsent` 同时断言完整 HTTP 状态、完整 body，并在之后回读宿主最终值；不能只收集含 `"won"` 的响应（`tests/cli.rs:1959`）。
- 失败写入后，旧值与旧 TTL 仍存在（`src/state.rs:351`）。
- 宿主自动生成的请求日志不记录 state key/value；脚本通过 `ctx.log.*` 主动记录的内容仍由脚本自行脱敏。

## Why This Matters

字符串模板注入把“数据”提升成了“代码”。只在替换顺序或转义函数上打补丁，无法保证未来的新占位符不会重新解释用户数据；结构化绑定把两类输入在类型系统与执行阶段彻底分开。

worker 中的校验代码与待校验值运行在同一可修改的 JavaScript 环境里。捕获 primordials 可以防止脚本通过覆盖全局关闭诊断，但最终安全结论仍必须由宿主基于收到的数据重新计算。尤其在编码边界上，信任 worker 的字节数会让同一个值在两侧得到不同判定，既可能误拒绝合法值，也可能让容量账本与真实存储不一致。

最后，测试若只观察“成功”字符串，会把错误响应或错误写入掩盖成通过。契约级反事实必须同时锁定传输层结果与宿主最终状态，否则绿色测试不能证明 CAS、容量或错误保持语义。

## When to Apply

- 新增或修改任何从 JavaScript worker 跨 IPC 调用宿主能力的路径时，尤其是状态、上传、文件、网络 allowlist 等有副作用的能力。
- 向 Boa realm 注入 request、env、header、上传元数据或内部标记时；数据应通过宿主值绑定，而不是参与源码生成。
- 在 worker 内实现“方便报错”的预检时；同时确认宿主仍在提交前独立校验安全与资源约束。
- 评审对抗性测试时；检查测试是否覆盖全局被替换、数据形状类似模板占位符、跨语言编码差异和失败后的最终状态。
- 修改状态、缓存或其它跨请求宿主存储的提交生命周期时；明确哪些副作用在脚本失败后保留，哪些 per-request 资源必须丢弃。

## Examples

### 结构化注入，而不是字符串替换

```rust
let request = JsValue::from_json(request, &mut context)?;
context.register_global_property(
    js_string!("__sd_request"),
    request,
    Attribute::all(),
)?;
```

prelude 在构造 `ctx.request` 后删除临时全局：

```javascript
request: requestSnapshot(__sd_request),
// ...
delete globalThis.__sd_request;
```

即使请求 query、header、body 或环境变量恰好包含内部占位符文本，它们也不会被后续替换。

### 捕获后调用 primordial

```javascript
var __sd_json_stringify = __sd_intrinsics.json_stringify;
var __sd_number_is_finite = __sd_intrinsics.number_is_finite;
var __sd_array_is_array = __sd_intrinsics.array_is_array;
```

后续 state 校验使用这些局部引用；用户脚本覆盖全局不会改变校验路径。

### 宿主重新计算 compact JSON 大小

```rust
fn validate_value(&self, value: &Json) -> Result<usize, StateError> {
    validate_json_value(value, 0)?;
    let compact = serde_json::to_vec(value).map_err(|_| StateError::InvalidInput)?;
    if compact.len() > MAX_VALUE_BYTES {
        return Err(StateError::InvalidInput);
    }
    Ok(compact.len())
}
```

worker 的预检负责尽早返回可捕获错误；真正决定是否写入的是宿主这次重新编码与检查。

## Related

- [host-boundary-fail-closed-input-classification.md](host-boundary-fail-closed-input-classification.md) — 宿主边界必须保留缺失、畸形与不可用状态，不能折叠成成功默认值。
- [script-worker-contract-max-and-post-deadline-held-slot-tests.md](script-worker-contract-max-and-post-deadline-held-slot-tests.md) — 每请求 worker、宿主能力与生命周期测试的既有约定。
- [blocking-upstream-body-to-async-stream-bridge.md](../architecture-patterns/blocking-upstream-body-to-async-stream-bridge.md) — per-request stream 只在成功结束后转移；可与宿主已提交的 state 写入对照。
- [refresh-documentation-code-anchors-by-semantics-not-offset.md](refresh-documentation-code-anchors-by-semantics-not-offset.md) — `src/script.rs` 大幅增行时，按语义 token 重新定位证据。
- [ADR 0021](../../../plans/adr/0021-v2-shared-state.md)、[`ctx` API 契约](../../contracts/ctx-api.md)、[`SECURITY.md`](../../../SECURITY.md) — Shared state 的容量、错误码、安全模型与公开语义。
- GitHub: [issue #176](https://github.com/geeknonerd/stuntdouble/issues/176)、[issue #175](https://github.com/geeknonerd/stuntdouble/issues/175)。

# Host crypto API：最小、可审计的加密原语

- 状态：`已接受`
- 日期：2026-10-22
- 关联：[ADR 0004](0004-host-functions-only-sandbox.md)、[ADR 0011](0011-contract-compatibility.md)、[ADR 0016](0016-v1-ctx-pending-capabilities-deferred.md)、[Post-v1 开发路线](../post-v1-roadmap.md)、[OAuth authorization mock](../oauth-authorization-mock.md)

## 背景

OAuth authorization mock 需要安全随机值、对原始请求 Body 计算 SHA-256、计算 HMAC-SHA256，并以不泄露比较位置的方式验证签名。当前脚本只能使用 Boa 提供的 JavaScript 能力：`Math.random()` 不是密码学安全随机源，纯 JavaScript 的哈希与比较也没有可用于生产语义的常数时间保证。

把上述能力留给每个脚本自行实现，会重复引入安全缺陷，并让不同 demo 对同一请求产生不同结果。引入通用外部 crypto 服务又会破坏“离线、零外部运行时依赖”的产品边界。因此需要一个范围足够小、输入边界明确、可以逐项审计和做已知答案测试的 host API。

## 决策

### API 形状

在 `ctx.crypto` 下只提供四项能力：

| API | 返回值 | 语义 |
| --- | --- | --- |
| `randomBytes(byteLength)` | 新的 `Uint8Array` | 从操作系统 CSPRNG 读取指定字节数 |
| `sha256(input)` | 32 字节 `Uint8Array` | 计算 SHA-256 |
| `hmacSha256(key, message)` | 32 字节 `Uint8Array` | 用给定 key 计算 HMAC-SHA256 |
| `timingSafeEqual(left, right)` | `boolean` | 比较两个字节序列；长度不同返回 `false`，等长时不按字节短路 |

公共输入类型为：

```ts
type SdCryptoInput = string | Uint8Array | ArrayBuffer | readonly number[];
```

- 字符串按 UTF-8 编码；含孤立 UTF-16 surrogate 的字符串抛 `crypto_invalid_input`，不静默替换字符。
- byte array 的每个元素必须是 `[0, 255]` 内的整数。
- 所有输入在进入宿主时复制；宿主返回新的 `Uint8Array`，脚本不能通过修改数组改变宿主缓冲。
- 长度一律按编码后的字节数计算，不按 JavaScript 字符数计算。

边界如下：

| 参数 | 允许范围 |
| --- | --- |
| `randomBytes.byteLength` | `1..=1024` |
| `sha256.input` | `0..=8 MiB` |
| `hmacSha256.key` | `0..=1 KiB` |
| `hmacSha256.message` | `0..=8 MiB` |
| `timingSafeEqual.left/right` | 各 `1..=1024` 字节；长度不相等时返回 `false` |

`timingSafeEqual` 只比较原始字节。脚本若需要比较十六进制字符串，必须自己按契约解码；宿主不会猜测字符串是 hex、base64 还是普通文本。

### 实现与依赖

- SHA-256 与 HMAC-SHA256 使用 RustCrypto 的 `sha2 0.10` 与 `hmac 0.12`。
- 安全随机使用 `getrandom 0.4`，直接读取操作系统熵源；不提供 `Math.random()` 降级路径。
- 常量时间比较使用 `constant_time_eq 0.6`。其 SPDX 表达式包含 Apache-2.0 选项，MSRV 低于本项目当前 MSRV。
- `sha2`、`hmac` 与 `getrandom` 均为 `MIT OR Apache-2.0`。实现 PR 更新 `Cargo.lock` 后必须同时通过 `deny`、`audit` 与 `msrv`。
- 不手写哈希、HMAC 或常量时间比较；不在 JavaScript 层补任何密码学实现。

标准库没有 SHA-256、HMAC、CSPRNG 或可依赖的常量时间比较 API。依赖图中的 `ring` 是 TLS 实现的内部依赖，不直接暴露为脚本契约，也不作为本决策的 API 提供者。

### 失败语义

- 参数类型错误、字节越界、长度超限或孤立 surrogate 由宿主统一抛可捕获的 `crypto_invalid_input`；hex/base64 等编码由脚本自行解码，宿主不猜测编码。
- 操作系统随机源失败抛可捕获的 `crypto_random_unavailable`。
- 未捕获的 crypto 错误是普通脚本错误，映射为 500 `script_error`；不新增 HTTP 状态，也不把底层操作系统错误暴露给客户端。
- 错误对象只含稳定 `code` 与固定说明，不包含输入、key、摘要、签名或宿主内部路径。

### 日志与隐私

- 宿主不记录 crypto 输入、key、随机输出、摘要或签名。
- 若后续为性能诊断增加 `crypto_calls` 日志，只允许记录操作名、输入/输出字节数和成功/失败类别；不得记录任何内容或内容哈希。
- `--verbose` 的 detail 也不得包含 crypto 参数值。

### 兼容性

这些 API 是对 `apiVersion` 1 的增量添加，依据 [ADR 0011](0011-contract-compatibility.md) 不提升 `apiVersion`。现有脚本不调用新成员时行为不变；类型定义与公开契约在实现落地的同一变更中更新。

## 后果

**正面**：OAuth 场景不再需要脚本自行发明安全随机、哈希、HMAC 或比较逻辑；四项能力都有明确长度上限和失败类别，可以分别用公开向量与边界测试锁定。宿主集中实现后，日志脱敏和依赖审计也只有一个落点。

**负面（接受）**：新增四个小型依赖，脚本 API 面略微扩大；输入在 worker 与宿主之间复制，最大 8 MiB 的消息会带来一次额外内存拷贝。该成本受现有 2 MiB 非 multipart 请求上限和 8 MiB buffered 读取上限约束。

**约束（不可破坏）**：crypto 只经 host function 提供，不向脚本暴露裸 `crypto`、`fetch` 或操作系统接口；不得以 JavaScript 实现替换宿主错误语义；不得把 key、Body 或签名写入日志。

## 验证义务

- SHA-256 至少覆盖空输入、`abc` 与含 NUL 的二进制输入。
- HMAC-SHA256 至少覆盖 RFC 4231 的公开向量、空消息与二进制 key。
- `timingSafeEqual` 覆盖等长相等、等长不等和不等长输入；不得用执行时间断言替代实现审阅。
- `randomBytes` 覆盖精确长度、上限、越界、连续两次结果不同，以及熵源失败路径。随机失败通过实现内的可注入熵源单元测试，不增加产品配置开关。
- 所有输入类型都覆盖合法与非法路径，并断言未捕获错误映射为 500 `script_error`。

## 替代方案

- **纯 JavaScript 实现**：无法可靠提供 CSPRNG 与常数时间比较，拒绝。
- **外部 crypto 服务或 sidecar**：增加网络、部署与信任边界，违背离线目标，拒绝。
- **暴露通用 `ctx.crypto` 套件**：范围过大，需要持续维护算法、编码、密钥格式与证书语义；只保留场景需要的四项能力。
- **使用 TLS 依赖 `ring` 作为统一提供者**：会把脚本契约绑定到内部 TLS 实现，依赖许可和封装面也不符合本决策，拒绝。

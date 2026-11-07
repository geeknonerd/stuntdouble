# 脚本文件写入：同根、显式开启、原子替换

- 状态：`已接受`
- 日期：2026-11-07
- 关联：[ADR 0001](0001-no-shared-state-in-v1.md)、[ADR 0004](0004-host-functions-only-sandbox.md)、[ADR 0011](0011-contract-compatibility.md)、[ADR 0021](0021-v2-shared-state.md)、[配置契约](../../docs/contracts/config.md)、[`ctx` API 契约](../../docs/contracts/ctx-api.md)、[安全说明](../../SECURITY.md)

## 背景

现有 `ctx.file` 只有 `readText`、`readBytes` 与 `stream`，`files.root` 被定义为只读静态文件根。产品定义明确把“脚本写文件”列为不做项。新的集成联调需求要求脚本把上传内容或生成内容写入配置的文件目录，并让后续请求能够读取或下载。

文件写入会产生跨请求可见、重启后仍存在的数据。若不先限定语义，它很容易被误用为 `ctx.state` 的替代品，与 ADR 0001 的“请求间不共享状态”和 ADR 0021 的“状态只能经宿主 API 读写”冲突。脚本也没有请求前/响应后生命周期 hook：worker 在产生最终结果后即退出，流式响应期间脚本已经结束。因此“请求时写入、响应后删除”不能靠现有脚本结构实现；它需要另一套宿主级清理能力。

## 决策

### 能力边界

文件写入是**显式 artifact output**，不是 Shared state：

- 不提供原子读改写、CAS、事务、TTL、订阅、跨实例协调或重启恢复承诺。
- 不保证多个请求并发写同一路径时的合并或顺序；同一路径由宿主按 last-writer-wins 处理。
- 脚本不能用它替代 `ctx.state` 实现一次性 code、token、防重放或跨请求协调；这些仍只能使用 `ctx.state`。
- 写入一旦完成即生效；后续脚本报错、超时、客户端断开或 worker 崩溃不会回滚。
- 文件不会自动清理；重启后保留。需要测试隔离时，部署方应为可写场景使用独立目录并在外部清理。

这是 v1.x 增量能力：配置仍为 version `"1"`，脚本 API 仍为 `apiVersion` `"1"`。它不提升产品 major 版本，但必须按 ADR 0011 同步公开契约、类型定义与发布说明。

### 配置

写入与读取共享同一个 `files.root`，但默认关闭：

```toml
[files]
root = "./files"
writable = true
write_max_bytes = 20971520
```

- `files.writable` 是布尔值，默认 `false`。关闭时任何写入 API 都抛可捕获的 `file_write_disabled`，不触碰磁盘。
- `files.write_max_bytes` 是每请求成功写入字节预算，默认 20 MiB，必须是正整数。
- 启用写入后，`root` 内的普通文件都可能被覆盖。需要保持 fixture 不可变的部署不应启用写入，或为其配置独立的 root。
- 领域术语从 `Static file root` 改为 `File root`；配置键 `files.root` 不变。

### API

在 `ctx.file` 下新增两个同步方法：

```ts
interface SdFile {
  writeText(path: string, text: string): number;
  writeBytes(
    path: string,
    bytes: Uint8Array | ArrayBuffer | readonly number[],
  ): number;
}
```

- 返回成功写入的字节数。
- `writeText` 按 UTF-8 编码；含孤立 UTF-16 surrogate 的字符串抛 `file_encoding_error`，不静默替换。
- `writeBytes` 只接受稠密字节容器；元素必须是 `[0, 255]` 内的整数。非法类型或元素是普通脚本错误。
- 不提供 append、delete、mkdir、rename、chmod、stream copy 或任意 mode 参数。大文件流式落盘需要单独的宿主能力与契约，不能在后续实现中悄悄扩大本 API。

### 写入语义

- 路径相对于 `files.root`；拒绝空路径、绝对路径、任何 `..` 组件、结尾的路径分隔符与最终 `.` 组件。
- 父目录必须已经存在；缺失父目录是 `file_io_error`，宿主不自动创建目录。
- 目标不存在时创建普通文件；目标存在时必须是普通文件，并被原子替换。
- 原子替换使用目标目录内的临时文件加 rename；不调用 `fsync`。失败时临时文件由 guard 清理，目标保持旧内容。
- 读取方只会看到旧文件或新完整文件，不会看到截断到一半的新文件。
- 多个请求并发写同一路径时 last-writer-wins；不提供锁、队列或冲突错误。
- 替换会断开目标路径上的硬链接，因此不会通过硬链接改写 root 外的 inode；若实现改成原地写（截断后写入），该保证即告失效。
- 新文件与替换后的文件使用宿主临时文件的默认权限（Unix 上为 `0600`），脚本不能设置 mode。

### 路径与符号链接

- 读取路径继续允许解析到 root 内的符号链接；写入路径更严格：任何符号链接组件（包括父目录和最终目标）都抛 `file_path_invalid`。
- 写入只允许普通文件；目录、FIFO、设备或 socket 目标是 `file_io_error`。
- Windows 上额外拒绝 alternate data stream（路径组件含 `:`）、保留设备名（含带扩展名与上标端口的形式）、结尾的点或空格，以及 Windows 不允许的字符。
- 解析以规范化 root 为基准；父目录在创建临时文件前再次规范化并确认仍位于 root 内。
- 现有本地半信任模型接受残余 TOCTOU 窗口；`SECURITY.md` 记录该取舍与升级路径（`openat`/`O_NOFOLLOW`）。
- 不设全局目录配额。磁盘配额、卷大小与清理责任属于部署方；每请求预算只限制单个请求的最大写入量。

### 限额与错误

- 单次写入上限 8 MiB，与缓冲读取上限一致。
- 每请求按成功写入的字节累计；超过 `files.write_max_bytes` 抛 `file_too_large`。失败的写入不扣减预算。
- 稳定错误码：
  - `file_write_disabled`：`files.writable = false`；
  - `file_path_invalid`：空路径、绝对路径、`..`、符号链接组件、越界；
  - `file_too_large`：单次 8 MiB 上限或每请求预算超限；
  - `file_encoding_error`：`writeText` 收到孤立 surrogate；
  - `file_io_error`：父目录缺失、目标非普通文件、权限、磁盘满或其他 I/O 失败。
- 未捕获的写入错误仍是普通 500 `script_error`；不新增 HTTP 状态码。

### 日志

- 每次写入追加到 `file_calls`，`api` 为 `file.writeText` 或 `file.writeBytes`，成功时记录 `bytes` 与 `duration_ms`，失败时记录稳定 `error`。
- 文件路径、文件内容、客户端文件名与临时文件名绝不写入日志。

## 后果

**正面**：脚本可以把上传内容或生成内容写入同一个配置目录，后续请求无需额外上传即可读取；默认关闭与显式预算保持现有部署安全；原子替换避免读者看到半截文件。

**负面（接受）**：文件会跨请求、跨重启保留，测试隔离由部署方负责；启用写入后 root 内普通文件都可被覆盖；没有全局配额；Unix 上替换后的权限变为保守的 `0600`；大文件无法通过脚本直接流式落盘。

**约束（不可破坏）**：不得把文件写入包装成 `ctx.state` 的替代品；不得静默创建父目录；不得跟随符号链接写入；不得把路径或内容写入日志；不得在没有新 ADR 的情况下增加 append、delete、stream copy 或全局配额语义。

## 验证义务

- 默认 `writable = false` 时写入抛 `file_write_disabled`，磁盘不产生文件。
- 开启后 `writeText` / `writeBytes` 可创建、覆盖，并在后续请求中通过 `readText` / `readBytes` / `stream` 读取。
- 绝对路径、`..`、空路径、父目录缺失、目录目标、符号链接父组件与符号链接最终目标均按契约失败。
- 单次 8 MiB、每请求预算、孤立 surrogate 与非法字节输入均 fail closed，且失败不破坏旧文件。
- 并发写同一路径时最终文件是某个完整版本，不出现半截内容；并发读取不会看到中间状态。
- `file_calls` 记录写入 API、字节数、耗时与错误，但不含路径或内容。
- Windows、macOS、Linux 的替换与符号链接行为按平台能力验证；Windows 的 ADS 与保留设备名拒绝必须有单元测试，平台差异写入安全说明。

## 替代方案

- **在同一 root 上默认允许写入**：会让现有只读部署在升级后突然暴露写能力，拒绝。
- **独立 `files.write_root`**：需要额外定义读 overlay、可见性与清理语义；当前需求只需一个可写文件根，拒绝作为首版范围。
- **直接 truncate 后写入**：实现简单，但并发读者可能看到半截文件，且无法保证失败时保留旧内容，拒绝。
- **请求时写入、响应后删除**：现有脚本没有响应后生命周期 hook；流式响应期间脚本已退出。若要做，应新增宿主级请求临时工作区与清理器，属于另一项能力，拒绝混入本次文件写入。
- **用 `ctx.state` 保存文件内容**：容量、TTL 与重启清空语义不适合 artifact，拒绝。

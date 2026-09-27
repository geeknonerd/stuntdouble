# 进程隔离外部事实调研

- 调研日期：2026-09-27（Asia/Shanghai）
- 范围：`rlimit`/Windows Job Object Rust 封装、macOS `RLIMIT_AS` 语义、cargo-dist 多 binary 归档、Unix 父进程退出联动。
- 本文只记录外部事实与来源，不评估或选择进程隔离方案。

## 结论速览

| # | 结论 | 置信度 |
| --- | --- | --- |
| 1 | `rlimit` 最新版为 0.11.0，MIT；提供安全 Rust API 设置 `RLIMIT_AS` 与 `RLIMIT_CPU`，调用方无需写 `unsafe`；仓库仍在维护。 | 高 |
| 2 | `win32job` 最新版为 2.0.3，`MIT OR Apache-2.0`；其安全 API 只能设置 working set、kill-on-close 等，不能设置 `JOB_OBJECT_LIMIT_PROCESS_MEMORY`，也没有 `TerminateJobObject` 包装。 | 高 |
| 3 | macOS 确实执行 `RLIMIT_AS`，但限制的是 VM map 的虚拟地址空间，不是 RSS/物理内存；当前 VM 占用高于目标值时，`setrlimit` 会以 `EINVAL` 失败。 | 高 |
| 4 | cargo-dist 以 Cargo package 为 App 边界：同 package 的多个 binary 进入同一平台归档；不同 package 默认各自独立归档，不能合并成一个 App。 | 高 |
| 5 | Linux 可用 `PR_SET_PDEATHSIG`，但需原生调用并处理设置前竞态；macOS 无该接口，可用 `kqueue`/`EVFILT_PROC` 或管道 EOF；管道是最常见的跨平台退路，但必须清理重复写端。 | 高 |

## 1. `rlimit` crate

### 事实

- 最新版本：`0.11.0`，发布于 2026-02-01T17:07:53Z。
- 许可证：MIT。该许可证满足仓库“MIT OR Apache-2.0”约束中的 MIT 分支。
- MSRV：`1.65.0`。
- 仓库：`Nugine/rlimit`；GitHub 显示未归档，最近一次 push 为 2026-09-13。
- 公开的 `Resource::AS` 和 `Resource::CPU` 分别对应 `RLIMIT_AS` 和 `RLIMIT_CPU`。
- `pub fn setrlimit(resource, soft, hard) -> io::Result<()>` 是安全函数；crate 内部通过 `unsafe` 调用 libc，但调用方不需要写 `unsafe`。
- `prlimit(pid, ...)` 也是安全包装，但只在 Linux/Android 上编译；它可用于设置任意进程的限制。
- 维护状态：v0.11.0 后有依赖和 codegen 修复提交，最近提交日期为 2026-09-13；仓库有 1 个 open issue，未归档。

### 来源

- crates.io：<https://crates.io/crates/rlimit>
- 仓库：<https://github.com/Nugine/rlimit>
- `Cargo.toml`（版本、许可证、MSRV）：<https://github.com/Nugine/rlimit/blob/v0.11.0/Cargo.toml#L1-L13>
- 安全 `setrlimit` 实现：<https://github.com/Nugine/rlimit/blob/v0.11.0/src/unix.rs#L21-L43>
- Linux/Android-only `prlimit`：<https://github.com/Nugine/rlimit/blob/v0.11.0/src/unix.rs#L69-L87>
- `Resource::AS`、`Resource::CPU`：<https://github.com/Nugine/rlimit/blob/v0.11.0/src/resource/generated.rs#L5-L22>
- 提交历史：<https://github.com/Nugine/rlimit/commits/main>

### 限制

“安全 API”只表示 Rust 调用点不需要 `unsafe`。如果该调用发生在 `CommandExt::pre_exec` 中，仍受 `pre_exec` 的 post-fork/async-signal-safe 约束，见第 5 节。

## 2. Windows Job Object Rust 封装：`win32job`

### 事实

- 最新版本：`2.0.3`，发布于 2025-05-15T10:25:16Z。
- 许可证：`MIT OR Apache-2.0`。
- 仓库：`ohadravid/win32job-rs`；GitHub 显示未归档。最近一次提交为 2025-06-29；截至 2026-09-27，发布版后没有新的代码提交。
- 安全 API 提供的 limits 包括：
  - `JOB_OBJECT_LIMIT_WORKINGSET`，通过 `limit_working_memory(min, max)` 设置；
  - `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`，通过 `limit_kill_on_job_close()` 设置；
  - breakaway、priority class、scheduling class、affinity。
- 不提供 `JOB_OBJECT_LIMIT_PROCESS_MEMORY` 的 setter：
  - `limits.rs` 没有导入该 flag 或 `ProcessMemoryLimit`；
  - `ExtendedLimitInfo` 内部 `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` 字段是 `pub(crate)`，调用方不能直接改字段补上该 flag。
- 工作集上限不是 process memory limit。crate README 明确写明：设置 working-memory 后“仍可继续分配内存，只是会被换页”。
- 不提供 `TerminateJobObject` 包装。它提供 `assign_process`/`assign_current_process`，并通过 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 在最后一个 job handle 关闭时终止已关联进程；`Job::drop` 会关闭 handle。
- `Job::handle()`/`into_handle()` 暴露底层 job handle，可与 `windows` crate 的原始 Win32 绑定衔接；原始 `SetInformationJobObject`/`TerminateJobObject` 调用属于 unsafe API。
- 仓库 issue #6“Implement more limits”自 2024-01-26 起保持 open，正文说明当前只实现了限制集合的子集。

### 操作系统能力对照

- Microsoft 文档定义 `JOB_OBJECT_LIMIT_PROCESS_MEMORY` 为“限制进程可提交的虚拟内存”；超过 per-process limit 的 commit 操作会失败。
- `TerminateJobObject` 可直接终止 job 中当前关联的全部进程，且进程无法延迟或处理该终止。
- 因此，不能设置 process memory limit 和不能显式 terminate 是 `win32job` 封装层的缺口，不是 Windows Job Object 本身的缺口。
- Microsoft 还说明：进程在关联到 job 之前已经执行的内存操作不会被 `AssignProcessToJobObject` 回查。

### 来源

- crates.io：<https://crates.io/crates/win32job>
- 仓库：<https://github.com/ohadravid/win32job-rs>
- `Cargo.toml`（版本、许可证）：<https://github.com/ohadravid/win32job-rs/blob/v2.0.3/Cargo.toml#L1-L35>
- 安全 limits 与方法：<https://github.com/ohadravid/win32job-rs/blob/v2.0.3/src/limits.rs#L14-L119>
- job 创建、分配、handle、drop：<https://github.com/ohadravid/win32job-rs/blob/v2.0.3/src/job.rs#L26-L108>
- README 对 working set 的说明：<https://github.com/ohadravid/win32job-rs/blob/v2.0.3/README.md#L14-L29>
- issue #6：<https://github.com/ohadravid/win32job-rs/issues/6>
- commits：<https://github.com/ohadravid/win32job-rs/commits/main>
- Microsoft `JOB_OBJECT_LIMIT_PROCESS_MEMORY`：<https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_basic_limit_information#job_object_limit_process_memory>
- Microsoft `JOBOBJECT_EXTENDED_LIMIT_INFORMATION`：<https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_extended_limit_information>
- Microsoft `TerminateJobObject`：<https://learn.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-terminatejobobject>
- Microsoft `AssignProcessToJobObject`：<https://learn.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-assignprocesstojobobject>

## 3. macOS 上 `RLIMIT_AS` 的实际语义

### 结论

- macOS 确实执行 `RLIMIT_AS`，但它是 VM map 的虚拟地址空间上限，不是 RSS、驻留内存或物理内存上限。
- `mmap` 路径和 malloc large allocator 的大块分配路径都会进入 Mach VM map 建立路径；新映射使 VM map 总量超过 `size_limit` 时，内核返回 `KERN_NO_SPACE`，对用户表现为映射/分配失败。
- 如果目标值小于进程当前 VM map 大小，降低 `RLIMIT_AS` 会失败：`vm_map_set_size_limit` 返回 `KERN_FAILURE`，BSD `dosetrlimit` 将其转换为 `EINVAL`。
- macOS 头文件把 `RLIMIT_RSS` 定义为 `RLIMIT_AS` 的别名，因此 macOS 不能分别设置 RSS 与地址空间限制；当前 XNU 实现中实际检查的是 VM map 大小。
- Apple 归档的旧 man page 仍把 `RLIMIT_RSS` 描述为物理内存/RSS 限制，这与当前头文件和 XNU 实现不一致，属于文档漂移，不能据此推断当前内核语义。

### 代码证据链

- XNU `resource.h`：
  - `RLIMIT_AS` 为 5，注释同时写了 “address space (resident set size)”；
  - `RLIMIT_RSS` 被 `#define` 成 `RLIMIT_AS`。
  - 来源：<https://github.com/apple-oss-distributions/xnu/blob/xnu-12377.121.6/bsd/sys/resource.h#L509-L511>
- `dosetrlimit(RLIMIT_AS)` 调 `vm_map_set_size_limit(current_map(), newrlim->rlim_cur)`；若限制低于当前 VM map 大小则报 `EINVAL`。
  - 来源：<https://github.com/apple-oss-distributions/xnu/blob/xnu-12377.121.6/bsd/kern/kern_resource.c#L1647-L1653>
- `vm_map_set_size_limit` 在 `new_size_limit < map->size` 时返回 `KERN_FAILURE`。
  - 来源：<https://github.com/apple-oss-distributions/xnu/blob/xnu-12377.121.6/osfmk/vm/vm_map.c#L19756-L19797>
- `vm_map_enter` 在建立映射后检查 `map->size > map->size_limit`，超限则把结果改为 `KERN_NO_SPACE`。
  - 来源：<https://github.com/apple-oss-distributions/xnu/blob/xnu-12377.121.6/osfmk/vm/vm_map.c#L3903-L3928>
- `mmap` 的匿名映射调用 `mach_vm_map_kernel`，file-backed 映射调用 `vm_map_enter_mem_object_control`。
  - 来源：<https://github.com/apple-oss-distributions/xnu/blob/xnu-12377.121.6/bsd/kern/kern_mman.c#L803-L810>
  - 来源：<https://github.com/apple-oss-distributions/xnu/blob/xnu-12377.121.6/bsd/kern/kern_mman.c#L899-L906>
- Apple libmalloc 的 large allocator 调 `mvm_allocate_pages`；后者调 `mach_vm_map`。
  - 来源：<https://github.com/apple-oss-distributions/libmalloc/blob/libmalloc-812.100.31/src/magazine_large.c#L720>
  - 来源：<https://github.com/apple-oss-distributions/libmalloc/blob/libmalloc-812.100.31/src/vm.c#L256-L318>

### Linux 差异

- Linux man page 明确定义 `RLIMIT_AS` 为进程虚拟内存（地址空间）上限，影响 `brk(2)`、`mmap(2)`、`mremap(2)`；超限分配返回 `ENOMEM`。这一点与 macOS 的“VM map 大小上限”在概念上接近。
- Linux 明确允许把 soft limit 降到低于进程当前已消耗值；后续继续增加消耗会被阻止。macOS 当前实现不允许新限制低于当前 VM map 大小，直接返回 `EINVAL`。
- Linux 的 `RLIMIT_RSS` 在 2.4.30 之后基本无实际限制效果；macOS `RLIMIT_RSS` 是 `RLIMIT_AS` 别名，二者不是等价语义。
- Linux 有 `prlimit(2)`，父进程可对指定 pid 设置 limit；`rlimit` crate 仅在 Linux/Android 暴露该安全包装。macOS 没有这个通用 `prlimit` 路径。
- Linux 来源：<https://man7.org/linux/man-pages/man2/getrlimit.2.html>
- Linux “降低 soft limit 可成功”与非 Linux `prlimit` 说明：<https://man7.org/linux/man-pages/man2/setrlimit.2.html>

### 已知坑

1. 低上限可能根本无法设置。macOS 进程启动后会保留大量虚拟地址区域；XNU 在设置时用当前 `map->size` 做下限检查。一个 2026-09-24 的实测 issue 报告 macOS 15.6/arm64 上 1 GiB 和 64 GiB 的 `RLIMIT_AS` 被拒绝，进程已有约 391 GiB virtual size，约 512 GiB 才能接受；这与上述内核检查一致。该 issue 是低置信度旁证：<https://github.com/ivzx04/SciFlawBenchHarness/issues/11>。
2. `RLIMIT_AS` 不是“用了多少物理内存”的硬上限。它可能因文件映射、保留区、malloc arena 等虚拟占用而提前失败；反过来，已存在的映射内继续触碰页面增加 RSS 不会触发新的 `vm_map_enter` 检查。
3. 大块 `malloc`/`mmap` 会受影响，因为 large allocator 会新建 VM mapping；但任何“真实内存上限”结论都不能只由 `RLIMIT_AS` 的数字推出。
4. 对已有进程设置很低的上限不可行；上限值与当前 VM 布局强相关，而不是只与实际常驻内存相关。

## 4. cargo-dist 多 binary 归档

### 结论

- cargo-dist 的 App 边界是 Cargo package，不是 workspace。
- 同一个 Cargo package 定义多个 binary：dist 把整个 package 当成一个 App，所有 binary 都进入该 App 在每个平台生成的同一归档和 installer。官方文档明确说该行为不可覆写。
- 同一个 workspace 中不同 Cargo package 各自定义 binary：每个 package 是一个独立 App，生成独立的 zip/安装器；官方文档明确说当前不能把多个 package 合并成一个 App。
- 归档会包含 App 的全部 binary，binary 位于归档根目录；额外静态文件可通过 `include` 放入归档。

### 来源

- 多 binary 同 package、不同 package 规则：<https://github.com/axodotdev/cargo-dist/blob/v0.33.0/book/src/workspaces/workspace-guide.md#multiple-binaries-in-one-package>
- 同一规则在 concepts 文档中的表述：<https://github.com/axodotdev/cargo-dist/blob/v0.33.0/book/src/reference/concepts.md#defining-your-apps>
- 归档内容：<https://github.com/axodotdev/cargo-dist/blob/v0.33.0/book/src/artifacts/archives.md#archive-contents>
- v0.33.0 release：<https://github.com/axodotdev/cargo-dist/releases/tag/v0.33.0>

### 对“sidecar 方案分发成本”的事实边界

- sidecar 与主程序在同一 Cargo package：按 cargo-dist 当前语义，不增加单独归档，二者在同一个归档/安装器中。
- sidecar 是 workspace 内另一个 Cargo package：默认增加一个独立归档/安装器，且当前 cargo-dist 没有把多个 package 合并为一个 App 的配置。

## 5. Unix 父进程死亡后的子进程处理

### Linux `PR_SET_PDEATHSIG`

- `PR_SET_PDEATHSIG` 只存在于 Linux：调用进程在父线程死亡时收到指定信号；设置为 0 表示清除。
- 父进程在这里按“创建子进程的线程”解释。该父线程退出即可触发，不需要整个父进程的所有线程退出。
- 该设置会在 `fork` 出的孙进程中清除；普通 `execve` 会保留，但执行 set-user-ID、set-group-ID 或带 capabilities 的 binary 时可能被清除，凭据变化也会清除。
- 竞态：如果父线程在 `prctl(PR_SET_PDEATHSIG, ...)` 之前已经死亡，调用不会补发信号。systemd 的标准处理是设置后再读 `getppid()`，若与预期父 PID 不同则自行 `raise` 目标信号并退出。
- Rust：`CommandExt::pre_exec` 是 `unsafe fn`，闭包在 `fork` 之后、`exec` 之前运行，只能做 async-signal-safe 操作。`libc::prctl` 本身也是 unsafe FFI。
- 来源：
  - Linux man page：<https://man7.org/linux/man-pages/man2/PR_SET_PDEATHSIG.2const.html>
  - Rust `pre_exec`：<https://doc.rust-lang.org/1.98.1/std/os/unix/process/trait.CommandExt.html#tymethod.pre_exec>
  - systemd 的 set-then-check 模式：<https://github.com/systemd/systemd/blob/v262/src/basic/process-util.c#L1637-L1678>

### macOS

- macOS 没有 `PR_SET_PDEATHSIG`。Apple 的 `kqueue(2)` 提供 `EVFILT_PROC` + `NOTE_EXIT`，可监视指定 PID 的退出事件。
- 若父进程已经退出，注册 watcher 前仍存在竞态，需要额外检查父 PID/生命周期状态；该接口不是 Linux PDEATHSIG 的语义等价替换。
- Apple 归档 man page：<https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/kqueue.2.html#//apple_ref/doc/man/2/kqueue>

### 跨平台 IPC 管道 EOF

- 常见模式：父进程保留 pipe 写端，子进程持有读端；父进程死亡时 OS 关闭其文件描述符，最后一个写端关闭后子进程 `read` 返回 EOF。
- POSIX 语义：所有指向写端的文件描述符关闭后，读端 `read` 返回 0。
- Windows anonymous pipe：`ReadFile` 在所有写 handle 关闭或出错时返回；进程终止会关闭其全部 handle，因此同样可实现“父进程死亡 => 子进程看到 EOF”。
- 主要坑：任何其他进程/线程继承或复制了写端，EOF 都会延迟；需要 close-on-exec、显式关闭未使用端，并限制 Windows handle inheritance。POSIX `pipe()` rationale 专门警告其他子进程泄漏写端会破坏 EOF 语义。
- 来源：
  - POSIX `pipe()`：<https://pubs.opengroup.org/onlinepubs/9799919799/functions/pipe.html>
  - Linux `pipe(7)`：<https://man7.org/linux/man-pages/man7/pipe.7.html>
  - Windows anonymous pipes：<https://learn.microsoft.com/en-us/windows/win32/ipc/anonymous-pipe-operations>

## 证据使用说明

- “高”表示结论直接来自 crates.io/上游源码、Apple XNU 源码、平台官方文档或项目官方文档。
- “中”表示结论由一手资料推导，但实际行为可能随进程布局、分配器或发行版补丁变化。
- 低置信度内容已单独标注，例如 macOS 实测 issue。

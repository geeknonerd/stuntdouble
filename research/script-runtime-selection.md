# Mock Server 脚本运行时选型（JS/TS + Python）

> 日期：2026-09-18  
> 关联：[ADR 0003](../plans/adr/0003-script-first-multi-runtime.md)、architecture-best-practices.md

## 一、核心结论

### JavaScript/TypeScript 运行时对比

| 方案 | TS 支持 | npm 包生态 | 安全模型 | 二进制大小 | 推荐场景 |
| --- | --- | --- | --- | --- | --- |
| **Node.js >=22.18** | ✅ 内建类型剥离（默认启用），稳定版 v25.2+ | ✅ 全量 | `--permission` 模型稳定，但 `--allow-net` 无域名粒度，仅能全开或全关；`--allow-fs-read=/path` 路径粒度可用 | 需要 Node 已安装，或捆绑 ~100MB | **首选**。npm 生态成熟，文档丰富，类型剥离无需编译；网络权限需靠代码层面白名单过滤。 |
| **Deno 1.x** | ✅ 原生支持（运行即剥离） | ✅ `npm:` specifier 兼容 | ✅ 强沙箱：默认无访问，`--allow-net=domain` 域名级、`--allow-read=./dir` 路径级；可 runtime prompt | 单二进制 ~100MB，可下载/捆绑 | **次选**。安全性最强（域名限流），但 npm 包兼容性和工具链不如 Node 成熟。 |
| **嵌入式引擎（rquickjs/Boa）** | ❌ 需前端转译（tsc/swc/esbuild） | ❌ 无 npm（需静态打包少量内置模块） | ✅ 纯进程内，可细粒度控制能力；无外部依赖 | 0（作为 Rust crate 嵌入） | 追求零系统依赖；但需要自行实现包解析和权限控制；对开发体验不友好。 |
| **Wasm（wasmtime/wasmer + JS→Wasm）** | ✅ 通过 transpile | ⚠️ 受限（Javy/AssemblyScript） | ✅ 强隔离 | 小 | 用于不可信第三方脚本；生态弱。 |

### Python 运行时对比

| 方案 | 优点 | 缺点 | 推荐度 |
| --- | --- | --- | --- |
| **子进程调用 python3** | ✅ 真实隔离（kill 即终止），便于设置 timeout/memory limits；✅ pip 生态完整 | ⚠️ 依赖用户安装 python3 ≥3.9；⚠️ IPC 开销；⚠️ 难做细粒度权限控制（FS/Network） | ⭐⭐⭐⭐ |
| **PyO3 嵌入 CPython** | ✅ 零启动开销；✅ 可直接导入本地函数 | ⚠️ 需要 libpython（或 bundled）；⚠️ GIL 影响并发；⚠️ 在进程中无法真正 sandbox（os/system calls 不受控） | ⭐⭐ |
| **RustPython（纯 Rust）** | ✅ 纯 RUST 实现，可嵌入 | ⚠️ stdlib/ecosystem 不完整（numpy/pandas 等缺失） | ⭐ |

---

## 二、技术事实核查

### Node.js TypeScript 支持

依据：[nodejs.org/typescript.html](https://nodejs.org/api/typescript.html)

- Type Stripping 行为：
  - v22.7.0 引入 `--experimental-transform-types` flag
  - v23.6.0 / v22.18.0：Type Stripping **默认启用**
  - v24.12.0 / v25.2.0：**Stable**，不再发 warning
  - v26.0.0：移除 `--experimental-transform-types` flag
  
- 限制：
  - Type Stripping 是轻量模式，仅去除类型注解，**不做类型检查**（type checking 需 tsc/bundled）
  - 不支持某些复杂 TS 语法（enums/namespaces 需要完整 transpile，建议用第三方包如 esbuild/ts-node）
  
- 权限模型（Stability 2 - Stable）：
  - Flags: `--permission` + `--allow-fs-read`, `--allow-fs-write`, `--allow-net`, `--allow-child-process`, `--allow-worker`, `--allow-addons`, `--allow-wasi`, `--allow-ffi`
  - 注意：`--allow-net` 为布尔值，**不支持域名粒度**（与 Deno 不同）
  - `--allow-fs-read=/path` 支持路径粒度
  - Audit mode 可用于发现权限需求

**结论**：Node.js 的 TS 支持已稳定且开箱即用，适合生产环境；权限模型对 network 只能"允许/不允许"二选一，需要通过应用层白名单进行域名过滤。

### Deno 安全模型

依据：[deno.com/runtime/fundamentals/security/](https://docs.deno.com/runtime/fundamentals/security/)

- Deno 默认沙箱：
  - 默认无 FS、Network、Env、Subprocess 访问权限
  - 通过 `--allow-*` flags 显式授予
  - Support granular scoping:
    - `--allow-net=example.com`：允许访问指定域名
    - `--allow-read=./data`：只允许读取指定目录
  
- Native TypeScript 支持：直接运行 .ts，自动 strip types

- NPM 兼容：通过 `npm:` specifier 使用 npm 包

**结论**：Deno 的沙箱模型更精细，尤其是 Network 的域名粒度，对于需要"脚本只能访问特定上游"的需求来说是一个显著优势。

### PyO3 嵌入 Python

依据：[pyo3.rs/v0.23.1/](https://pyo3.rs/v0.23.1/)

- PyO3 通过绑定 CPython C API 嵌入 Python 解释器到 Rust
- 需要确保 Python 编译了共享库（例如 Ubuntu: `sudo apt install python3-dev`）
- 提供 `with_embedded_python_interpreter()` API
- **不会自动初始化 Python 解释器**，需要 `auto-initialize` feature 或通过手动 attach

**结论**：PyO3 适合高性能需求，但需要外部 Python 运行时支持，且在进程内无法真正 sandbox。

---

## 三、最终选型（2026-09-18 确认：全部内置，零外部环境依赖）

需求约束：单二进制交付、不依赖用户安装 Node/Python、运行时尽量轻量、少量常用内置库即可、暂不需要 import。

| 语言 | 选型 | 关键事实（已核验） |
| --- | --- | --- |
| **JS/TS** | **Boa**（`boa_engine` 0.22.0） | 纯 Rust 实现；官方 README 自述 experimental，覆盖最新 ECMAScript 规范 90%+（test262 结果在 boajs.dev/conformance）；v0.22.0 发布于 2026-08-28 |
| JS WebAPI | **`boa_runtime`** | 提供 `fetch`（需 `fetch` feature，经 `extensions::FetchExtension` + `fetch::BlockingReqwestFetcher` 注入，底层 reqwest）、`interval`（setTimeout/clearTimeout/setInterval/clearInterval）、`console`、`URL`、`base64`、`AbortController`、`structuredClone`、`TextEncoder`、`process`；WinterTC/TC55 最小公共 Web API |
| TS 转译 | **swc_core** 或 **oxc_transformer** | 纯 Rust crate；只做类型剥离，不做类型检查；无外部工具链 |
| **Python** | **RustPython**（`rustpython-vm` 0.5.0） | 纯 Rust 解释器，README 自述对齐 CPython >= 3.14.0；MIT；有 WebAssembly demo；无 C 扩展（numpy/pandas 不可用） |

### 被否掉的方案与原因

- **子进程 Node/Python**：生态最好，但要求用户安装运行时（或捆绑 ~100MB 级运行时），与"轻量、零外部依赖"冲突。
- **QuickJS（rquickjs）**：体积更小、启动更快、非 experimental；但为 C 实现（含 WASM 支持），且需要自行绑定更多 API。若 Boa 的成熟度成为阻塞项，这是第一替补。
- **denort / 嵌入 V8**：TS 与 npm 原生支持最完整，但 V8 体积（数十 MB）与"尽量轻量"冲突。
- **PyO3 嵌入 CPython**：需要 libpython，且进程内无法真正限制 Python 的 os/socket 能力。
- **纯声明式模板**：已验证场景（外部取数 + 二进制转发）超出模板表达力。

### 安全边界的落点变化（重要）

嵌入式引擎失去进程隔离后，超时与内存上限不再是 `kill` 子进程，而必须由 Rust 宿主实现：Boa 侧用中断处理器（interrupt handler）与内存压力回调，RustPython 侧用执行步骤中断；脚本 panic 需 `catch_unwind` 兜住，否则爆炸半径是整个服务。

## 四、对产品设计的影响

1. **脚本 API 必须由产品自己定义**：没有 `require`、没有 `import requests`（除非产品内置同名封装）。因此"宿主提供哪些函数"从实现细节升级为**产品功能定义的核心部分**（下一个待收敛项）。
2. **`fetch` 的暴露方式**：boa_runtime 自带 fetch，但为满足域名白名单，产品应提供受限宿主函数（如 `ctx.http.get(url)`）而非裸 `fetch`。是否同时保留裸 `fetch` 需在 API 契约中明确。
3. **Python `requests`**：不能安装 pip 包，需要产品内置同名能力的封装模块。
4. **TS 是编译期体验**：类型剥离意味着运行时不报类型错误，类型检查完全依赖用户编辑器 + 产品发布的 `.d.ts`。
5. **升级路径**：若未来需要生态，可在同一 API 契约下把某个语言的宿主换成子进程运行时，对用户脚本尽量透明。

# 配置管理与动态能力架构（Best Practices）

> 日期：2026-09-18  
> 来源：本项目已验证的 [Mock 服务选型调研](./mock-server-landscape.md)，本次补充核查 WireMock、Mockoon、Prism 官方文档；Mountebank/MockServer/Hoverfly 参考项目历史验证结果。

## 一、三种架构模式

### 模式 1：静态配置（改文件 + 重启）

**特征**：
- 配置文件是事实源（Source of Truth，SOT）
- 进程启动时加载配置文件并构建路由表；任何变更需要重启或冷加载
- 无管理平面（Control Plane）暴露给外部调用者

**优点**：
- 确定性最高：CI 中复现性好，git 版本可审计
- 无安全面：没有写接口暴露给网络

**缺点**：
- 本地开发体验差：每次修改需要手动重启

**适配场景**：v1，以 CI 为主，本地为辅

---

### 模式 2：静态配置 + 自动热重载

**实现要点**：
1. **单一执行模型**：统一流程 `load → validate → rebuild route table`  
   - 所有来源（CLI、文件导入、Admin API）最终都编译为此流水线实例，不形成两个引擎
2. **原子性切换**：新路由表加载成功后再替换全局引用，失败保持旧状态
3. **错误诊断**：解析失败打印详细上下文，不中断服务
4. **性能边界**：大配置文件下加载时间需在可接受范围内（实测建议 < 50ms 级）

**优点**：
- 本地开发接近“运行时动态”但无需控制面复杂度
- 与管理 API 模式共享核心 `reload()` 入口，演进成本低

**缺点**：
- 仍可能有冷重启风险（如脚本依赖、资源句柄等）
- 需对大配置文件的加载性能进行测试

---

### 模式 3：管理 API（运行时增删改）

**典型实现**（参考 WireMock、Mockoon、Hoverfly、Mountebank、MockServer）：
- **内存 vs 磁盘双存储**：
  - WireMock：内存为当前生效态；用户显式调用 `POST /__admin/mappings/save` 落盘 → **明确“谁为真”** 问题 ([WireMock 官方文档](https://wiremock.org/docs/standalone/admin-api-reference/))
  - Mockoon：Admin API（路径 `/mockoon-admin`）保护 bearer token，支持数据变量、环境变量、数据桶（Data Buckets）、服务器状态的运行时管理 ([Mockoon 官方文档](https://mockoon.com/docs/latest/admin-api/overview/))
- **并发一致性**：乐观锁版本号、时间戳校验（业界通用）
- **控制面安全**：
  - 单独端口或限定 `localhost`
  - 基础鉴权（token/API Key）
  - 最小权限：只允许 mappings 操作，禁止文件系统访问

**优点**：
- 真正的运行时可编程能力
- 适合多租户、自动化环境造、CI 管道集成

**缺点**：
- 控制面复杂度高：鉴权、限流、审计、权限粒度
- 双 SOT 风险：需明确“内存 vs 磁盘谁为真”（推荐：磁盘 + dump 同步）
- 安全性要求高：Admin 端点必须网络隔离或强鉴权

---

## 二、各方案对比总结

| 产品 | 运行时改配置方式 | 是否热重载 | 配置持久化策略 | 控制面安全 | 备注 |
| --- | --- | --- | --- | --- | --- |
| **WireMock OSS** | `POST/PUT DELETE /__admin/mappings*` | 是，内存生效 | 显式 `save` 动作落盘 | `__admin` 端口需隔离/鉴权 | Java，状态机有限制 |
| **Mockoon** | CLI/Admin API (`/mockoon-admin`) | 是 | 环境变量/数据桶 | Bearer Token | MIT，JS Hooks |
| **Hoverfly** | `POST/PUT /api/v2/simulation` | 是 | simulation import/export | middleware 进程隔离 | Go，stateful pairs |
| **json-server v0.17** | 修改 db.json + 重启 | 否，手动重启 | 直接写回文件 | 无控制面，文件即 SOT | CRUD 自动派生 |
| **Prism** | OpenAPI 驱动，启动时指定合约 | 不适用 | 静态 OpenAPI | 通常配合反向代理控制 | 合约优先 |

---

## 三、推荐设计约束（第一版取模式 1，保留向模式 2/3 演进的入口）

### C1. 单一执行模型（延续 ADR 0002）

- 路由内部四段流水线：**匹配→源→变换→响应**
- 所有来源（CLI 参数、文件导入、Admin API、OpenAPI 合约）最终都编译为此流水线实例
- **约束**：不得出现第二个执行引擎（如自动 CRUD 引擎 + 自定义路由引擎）

---

### C2. 配置即事实源（Config-as-SOT）

- **第一版**：配置文件是唯一 SOT，运行时无写入路径
- **演进路线**（到 C）：
  - 内存配置只是缓存；提供 `config save` 将当前生效状态落盘
  - 冲突处理：保留操作日志，支持合并或覆盖策略
  - `reload()` 函数作为核心入口：无论是文件监听触发还是 Admin API 应用变更，都走同一路径

---

### C3. 声明式变换优先，脚本作为逃逸口

- **基础能力**（推荐全部支持）：
  - 路径参数映射（`:group`, `:document_id`）
  - JSONPath 字段选择（从上游响应提取字段）
  - 模板拼接（Handlebars/Jinja2/Go templates）
- **进阶能力**（可选开关）：
  - JavaScript/Python 钩子（类似 Mockoon Callbacks、Mountebank Inject）
  - 用于复杂计算、外部查询、条件分支
- **安全边界**：
  - 默认关闭脚本能力，需显式开启并记录审计日志
  - Sandbox 限制（禁用 `eval`、文件系统访问、网络请求白名单）

---

### C4. 控制面数据安全（为 C 做准备）

- Admin API **单独端口**或限定在 `localhost`
- **基础鉴权**（Token/API Key）
- **最小权限**：只允许 `GET/POST/PUT DELETE` mappings，禁止任意代码执行

---

### C5. OpenAPI 预设生成器（未来功能）

- 将 OpenAPI YAML/JSON 编译为普通路由实例
- 每个 `pathItem` 生成若干 pipeline 节点（GET/POST 等）
- **保留覆盖机制**：用户对生成的预设有完全控制权，能展开编辑成自定义路由
- **本质**：这是“路由模型的批量生成”，不是独立引擎

---

## 四、关键结论

1. **热重载是“动态配置”的最低成本形态**：不需要暴露任何控制面即可满足大部分本地开发需求。实现关键在于定义一个清晰的 `reload()` 入口和原子切换逻辑。

2. **管理 API 的本质是 Control Plane/Data Plane 分离**：数据面处理实际请求；控制面负责 mappings 的生命周期（创建/读取/更新/删除）。两者不应混用端口，且控制面必须有鉴权。

3. **“谁为事实源”的问题必须明确**：如果引入 C，则内存配置只是缓存。应提供 `config save` 将当前生效状态落盘，并支持冲突合并。否则会出现"内存中有更改的文件没保存"这种事故。

4. **声明式变换 + 脚本逃逸口是最优表达力边界**：简单场景纯声明式足够，复杂场景需要脚本能力。关键是明确边界并提供开关和安全沙箱。

---

## 五、待补充验证项（需后续补充）

以下链接未在本次会议中逐一核对，应在锁定实现前复核：
- Mountebank：[docs/api/injection](https://www.mbtest.org/docs/api/injection)、[docs/api/imposters](https://www.mbtest.org/docs/api/imposters)
- MockServer：[creating expectations](https://www.mock-server.com/mock_server/creating_expectations.html)、callbacks
- Hoverfly：[simulations API](https://docs.hoverfly.io/en/latest/pages/reference/api/api.html)、middleware、stateful patterns
- Prism：更多 OpenAPI mock 细节

---

## 六、参考文献

- WireMock Admin API Reference: https://wiremock.org/docs/standalone/admin-api-reference/
- Mockoon Admin API Overview: https://mockoon.com/docs/latest/admin-api/overview/
- Project internal: [Mock 服务选型调研](./mock-server-landscape.md)


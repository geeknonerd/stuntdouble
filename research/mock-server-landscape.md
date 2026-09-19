# 开源/免费 Mock 服务调研（动态配置、跨接口逻辑、文件 I/O）

> 调研日期：2026-09-12。目标是找到可在本地或 CI 部署、运行时增删接口，并能用代码编排多个接口（例如接口 A 从本地/外部取数，接口 B 再从 A 的数据取字段），同时覆盖文件上传和文件/二进制响应的方案。

## 重要说明：资料可访问性

初次调研时环境无法连接外网：本地代理端口未监听，直连 DNS 也失败。2026-09-12 追加复核时本地代理已间歇性可用，成功核对了 WireMock、Hoverfly、MockServer、Mockoon 和 MSW 的部分官方页面；其余链接与版本/API 仍应在锁定版本前复核。链接均指向项目维护的一级资料，不引用博客或二手文章。

## 结论摘要

* **首选 WireMock OSS**：Admin API 可热更新 stub；一等支持 multipart 上传匹配和 `__files` 文件/二进制响应；Scenario、模板和 Java 扩展可覆盖从简单状态到复杂跨接口编排。Apache-2.0，可 JAR/Docker 运行。
* **需要 JavaScript、轻量 Node 运行时可选 Mountebank**：REST 动态配置和 `inject(request, state)` 适合把多个接口写在一个脚本中，也有 proxy；但 multipart 和文件响应不是一等能力，需自己解析/编码，注入默认受安全开关限制。
* **MockServer** 适合 JVM 团队：REST/Java Client 动态 expectations、回调代码、forward/proxy 很强；文件响应可用 FileBody/BinaryBody。跨请求持久状态和 multipart 结构化匹配需要自行实现或核实版本。
* **Mockoon** 适合 GUI 驱动的快速原型：开源桌面端、CLI/Docker、运行时管理 API、模板和 JavaScript hooks。复杂跨接口数据管道及 multipart/file 语义依赖脚本，团队应先做 PoC。
* **Prism** 适合 OpenAPI 合约 mock/校验和代理，不适合作为需要任意业务代码、跨接口状态或文件传输的主 mock 引擎。

## 能力矩阵

| 项目 | 动态配置（运行时） | 代码实现跨接口逻辑/外部取数 | 文件上传 | 文件/二进制返回 | 许可证与部署 | 主要限制 |
|---|---|---|---|---|---|---|
| **WireMock OSS** | Admin API `POST/PUT/DELETE /__admin/mappings`，无需重启即可增删改；[Admin API](https://wiremock.org/docs/standalone/admin-api-reference/) | Scenarios 提供跨请求状态机；Response templating 读取请求/状态；proxy 转发上游；复杂逻辑通过 Java `ResponseDefinitionTransformer`/扩展。[Stateful](https://wiremock.org/docs/stateful-behaviour/)、[templating](https://wiremock.org/docs/response-templating/)、[proxy](https://wiremock.org/docs/proxying/)、[extensions](https://wiremock.org/docs/extensibility/transforming-responses/) | 一等 multipart matcher（按 part 名称、头、body 匹配），见[Request matching](https://wiremock.org/docs/request-matching/#multipart-form-data) | `bodyFileName` 从映射目录 `__files` 读取；`base64Body` 或文件均可，见[Returning a response from a file](https://wiremock.org/docs/stubbing/#returning-a-response-from-a-file) | Apache-2.0（[GitHub LICENSE](https://github.com/wiremock/wiremock/blob/master/LICENSE.txt)）；Standalone JAR、Docker（[Docker](https://wiremock.org/docs/standalone/docker/)） | Scenario 是有限状态；跨接口任意数据提取/写回需扩展或外部服务；`__admin` 端点应做网络隔离/鉴权；文件路径限定在 `__files` 目录 |
| **Mountebank** | REST `POST /imposters`、`PUT /imposters/:port`、`DELETE` 热更新；[API](https://www.mbtest.org/docs/api/mountebank) | `inject` JavaScript 函数接收 `request, state, logger`，可在内存 state 中共享数据；`proxy` 转发外部接口，`copy/lookup` 可复用请求字段/查表。[Injection](https://www.mbtest.org/docs/api/injection)、[behaviors](https://www.mbtest.org/docs/api/behaviors)、[proxy](https://www.mbtest.org/docs/api/proxies) | HTTP imposter 暴露原始 body；官方未提供 multipart 专用 matcher，需在 inject 中解析（body 可能是 base64） | `is` 响应主要是字符串 body；二进制需预编码或 inject 生成。无 `bodyFileName` 一等字段，文件读取受注入沙箱/权限影响 | MIT（[GitHub LICENSE](https://github.com/bbyars/mountebank/blob/master/LICENSE)）；Node CLI `mb start`、Docker（[Quickstart](https://www.mbtest.org/docs/quickstart)） | `state` 仅内存、重启丢失；生产需显式评估 `--allowInjection` 安全风险；multipart、静态文件服务需自行编码 |
| **MockServer** | REST API/Java Client 创建、更新、清除 expectations，立即生效；[Creating expectations](https://www.mock-server.com/mock_server/creating_expectations.html) | Java `ExpectationResponseCallback`/`ExpectationForwardCallback` 可执行自定义代码；forward/proxy 访问外部服务；模板可按请求动态渲染。[Callbacks](https://www.mock-server.com/mock_server/creating_expectations.html#button_response_class_callback)、[forwarding](https://www.mock-server.com/mock_server/creating_expectations.html#forward_action)、[templates](https://www.mock-server.com/mock_server/response_templates.html) | 支持 String/Binary/JSON 等 body matcher；multipart 结构化匹配能力和版本相关，建议 PoC 验证（[Creating expectations → request matchers](https://www.mock-server.com/mock_server/creating_expectations.html#request_matchers)） | `HttpResponse` 的 `BinaryBody`，以及 `FileBody`/模板读取文件（[Creating expectations → response actions](https://www.mock-server.com/mock_server/creating_expectations.html#response_action)、[File storage](https://www.mock-server.com/mock_server/file_storage.html)） | Apache-2.0（[GitHub LICENSE](https://github.com/mock-server/mockserver-monorepo/blob/master/LICENSE.md)）；Standalone JAR、Docker（[Running](https://www.mock-server.com/mock_server/running_mock_server.html)） | 回调类需放入 JVM classpath，部署复杂；跨请求共享状态需自行维护，forward 不会自动把 A 响应注入 B |
| **Mockoon** | 桌面端编辑环境/路由；CLI 提供环境管理和 Admin API（[CLI](https://mockoon.com/cli/)；[Admin API](https://mockoon.com/docs/latest/admin-api/overview/)）；适合开发时热改配置 | Handlebars 模板、faker/data buckets 和 JavaScript hooks 可读写请求/响应并调用自定义逻辑（[Data buckets](https://mockoon.com/docs/latest/data-buckets/overview)；[动态规则](https://mockoon.com/docs/latest/route-responses/dynamic-rules/)）；可将上游请求交给脚本/代理（具体 API 以版本为准） | 路由可接收原始请求 body；multipart 字段解析、持久化文件需 hook 自行实现 | 支持从文件生成响应（[File serving](https://mockoon.com/docs/latest/response-configuration/file-serving/)）；二进制头/编码需自行设置，复杂场景先 PoC | MIT（[GitHub LICENSE](https://github.com/mockoon/mockoon/blob/main/LICENSE.md)）；桌面应用、CLI、Docker | 复杂跨接口状态和外部 HTTP 调用依赖 JavaScript hook；不同发行版的文件 helper/API 可能不同；不如 WireMock 有成熟 multipart DSL |
| **Prism** | CLI 启动时加载 OpenAPI；通过 proxy/mocking 参数切换行为，配置以规范文件为中心（[Prism mocking](https://docs.stoplight.io/docs/prism/5c12e4f1b6f4d-mocking)） | 根据 OpenAPI 示例/模式生成响应并可代理上游；无通用跨接口脚本状态机 | 仅按 OpenAPI schema 校验请求；无专用文件上传存储/解析工作流 | 可描述 `application/octet-stream` 等 schema，但静态/动态文件读取需外部服务 | Apache-2.0（[GitHub LICENSE](https://github.com/stoplightio/prism/blob/master/LICENSE)）；Node CLI、Docker | 规范驱动而非业务脚本引擎；不满足“接口 A 取数→接口 B 提取”的复杂编排 |

## 典型实现草图

### WireMock：multipart + 文件响应 + 跨请求状态

1. 通过 Admin API 注册映射：

```json
{
  "request": {
    "method": "POST", "urlPath": "/upload",
    "multipartPatterns": [{"matchingType":"ANY","name":{"equalTo":"file"}}]
  },
  "response": {"status": 201, "jsonBody": {"id":"{{randomValue length=8 type='ALPHANUMERIC'}}"}, "transformers":["response-template"]},
  "scenarioName": "files", "requiredScenarioState": "Started", "newScenarioState": "Uploaded"
}
```

2. 后续 `GET /metadata` 映射设置 `requiredScenarioState: Uploaded`，用模板返回请求/状态数据。若需把上传内容持久化、再由另一个接口查询，应实现 Java transformer（或让外部测试数据服务持久化）；Scenario 本身只保存有限状态名。
3. 文件下载映射使用 `"bodyFileName":"report.pdf"`，将文件放在同一实例的 `__files/report.pdf`；二进制可用 `base64Body`。管理端点应仅对测试网络开放。

### Mountebank：inject 中维护接口间内存 state

```json
{
  "port": 4545, "protocol": "http",
  "stubs": [{
    "predicates": [{"equals": {"method":"POST", "path":"/upload"}}],
    "responses": [{"inject": "function (request, state) { state.file = request.body; return { statusCode: 201, body: JSON.stringify({id:'x'}) }; }"}]
  }, {
    "predicates": [{"equals": {"method":"GET", "path":"/metadata"}}],
    "responses": [{"inject": "function (request, state) { return { statusCode: 200, body: JSON.stringify({size:(state.file||'').length}) }; }"}]
  }]
}
```

启动时需确认注入开关（`--allowInjection`）及沙箱策略；state 在进程重启后清空。文件下载应将内容预先 base64 编码放入 `is.body`，或在受控注入中读取文件并设置 `Content-Type`。

### MockServer：回调代码 + 外部转发

通过 `PUT /mockserver/expectation` 注册 expectation，`httpResponse` 可用 `body`/`file`/`binary`；需要跨接口逻辑时实现 `ExpectationResponseCallback`，在回调中调用外部 HTTP 客户端并维护共享缓存，再返回 `HttpResponse`。MockServer 还提供 stateful scenarios（按顺序/次数切换响应），但任意业务数据仍需回调或外部存储。回调类需打包到 MockServer JVM classpath，CI 镜像应固定依赖版本。

## 选型建议（按需求权重）

| 需求 | 建议 |
|---|---|
| 文件上传匹配、文件下载、运行时 API 三者都要开箱即用 | WireMock OSS |
| 主要是 JavaScript 编排、接受自行处理 multipart/文件 | Mountebank |
| 团队已有 JVM、需要 Java 回调/代理 | MockServer |
| 产品/前端同学通过 GUI 快速搭环境，逻辑较简单 | Mockoon |
| 以 OpenAPI 合约校验为核心，逻辑由真实上游提供 | Prism |

建议先用 WireMock 做最小 PoC：验证“上传→提取 ID→查询→下载”四步和并发隔离；若必须在单脚本中自由调用外部 API，再对比 Mountebank inject 与 MockServer callback 的安全、持久化和可测试性。

## 复核清单

1. 锁定版本并在 CI 下载对应官方发行包，检查 Apache-2.0/MIT 义务及第三方依赖。
2. 用真实 `multipart/form-data`（文本字段、大文件、重复字段）验证匹配器和大小限制。
3. 验证跨接口状态在并发测试、实例重启、测试重试时的隔离/清理策略。
4. 将 WireMock `__admin`、Mountebank injection、MockServer callback 端口限制在测试网络，避免把任意代码执行面暴露出去。

## 增补调研（2026-09-12）

> 网络复核：本次运行本地代理仍未监听，直连 DNS 也不可用。以下新增条目严格按项目维护的官方文档、API 参考和 GitHub 源码/许可证链接整理；在锁定版本前应重新打开链接确认 API 是否有变更。

### Hoverfly

[Hoverfly](https://hoverfly.io/) 是 SpectoLabs 维护的 Go 服务虚拟化工具，支持 capture（录制真实流量）和 simulate（按 simulation 返回响应）两种常用模式。运行中的 simulation 可以通过官方 REST API 查询、替换、导入和导出（`/api/v2/simulation`），因此不需要重启进程就能切换场景：[API reference](https://docs.hoverfly.io/en/latest/pages/reference/api/api.html)、[Simulation](https://docs.hoverfly.io/en/latest/pages/keyconcepts/simulations/simulations.html)。项目提供单二进制、Docker 等发行方式，仓库许可证为 Apache-2.0：[GitHub repository](https://github.com/SpectoLabs/hoverfly)、[LICENSE](https://github.com/SpectoLabs/hoverfly/blob/master/LICENSE)。

跨接口编排可用 Hoverfly 的 stateful simulation：每个 request/response pair 可以声明 `requiresState` 和 `transitionsState`，用状态转换约束“先上传、后查询/下载”的顺序：[State](https://docs.hoverfly.io/en/latest/pages/keyconcepts/state/state.html)。对于需要任意代码（例如调用外部 API、把上传内容写入数据库）的场景，可配置 on-request/on-response middleware；middleware 是由 Hoverfly 启动的外部进程，通过 JSON stdin/stdout 修改请求或响应：[Middleware](https://docs.hoverfly.io/en/latest/pages/keyconcepts/middleware.html)。这比在服务内嵌脚本更容易隔离，但需要额外维护 middleware 进程和超时/错误策略。

文件能力的边界需要特别注意：simulation response schema 支持把响应体标记为 `encodedBody` 并携带 Base64 内容，适合返回 PDF、图片等二进制；也可以让 middleware 在运行时读取文件并生成该响应：[Simulation API/schema](https://docs.hoverfly.io/en/latest/pages/reference/api/api.html)。官方 request matching 文档没有提供类似 WireMock 的 multipart part matcher；multipart 上传通常只能作为原始 body 匹配，或交给 middleware 解析 `multipart/form-data` 后自行保存/校验：[Matching](https://docs.hoverfly.io/en/latest/pages/keyconcepts/matching/matching.html)。因此 Hoverfly 适合“录制/回放 + 少量状态”，但要完成“上传文件→提取字段→另一接口返回文件”，应把持久化和 multipart 解析放进受控 middleware，并为并发隔离设计 state key。

**最小配置思路**：先用 `PUT /api/v2/simulation` 上传包含 `requiresState`/`transitionsState` 的 simulation；上传接口的 on-response middleware 生成业务 ID 并写入外部测试存储，查询接口再由 on-request middleware 读取该存储；下载接口返回 `encodedBody: true` 的 Base64 文件。将 Hoverfly API 端口和 middleware socket 限制在测试网络内。

### Mock Service Worker（MSW）

[MSW](https://mswjs.io/) 是 MIT 许可的 JavaScript/TypeScript 请求拦截库（不是独立守护进程），同一套 handlers 可在浏览器 Service Worker 和 Node.js 中运行：[仓库 LICENSE](https://github.com/mswjs/msw/blob/main/LICENSE.md)、[Node setupServer](https://mswjs.io/docs/api/setup-server)、[Browser setupWorker](https://mswjs.io/docs/api/setup-worker)。它通过 `server.use(...)`/`worker.use(...)` 在运行时追加或覆盖 handler，并可用 `resetHandlers()` 恢复初始配置：[use](https://mswjs.io/docs/api/setup-server/use)、[resetHandlers](https://mswjs.io/docs/api/setup-server/reset-handlers)。这满足测试进程内的动态配置，但没有 WireMock 那样的远程 Admin API；若多个测试进程/语言共享 mock，需要自行封装控制服务或选择独立 mock server。

每个 handler 的 resolver 都能执行普通 JavaScript，因此可在模块级或依赖注入的 store 中保存接口 A 的结果，再由接口 B 查询；resolver 也可以显式调用外部 `fetch` 取得真实数据（注意避免再次命中同一 handler）：[Request handlers](https://mswjs.io/docs/concepts/request-handler)、[Response resolver](https://mswjs.io/docs/concepts/response-resolver)。MSW 将标准 Fetch `Request` 传给 resolver，上传接口可调用 `await request.formData()` 读取 `File`/字段并写入测试 store；这依赖运行时的 WHATWG FormData/File 实现，Node 版本和 polyfill 应在 CI 中固定。响应端使用官方 `HttpResponse`，可返回 `HttpResponse.arrayBuffer(...)`（二进制）、`HttpResponse.text(...)` 或 `HttpResponse.json(...)`，并自行设置 `Content-Type`/`Content-Disposition`：[HttpResponse API](https://mswjs.io/docs/api/http-response)。

**适用性判断**：如果被测应用与 mock 同为 JS/TS，MSW 能以最少基础设施实现复杂跨接口逻辑和文件 I/O，并可在单元/集成测试中按用例动态切换 handler；如果需要一个可被其他语言、移动设备或远程环境访问的独立 HTTP mock 服务，MSW 不应作为唯一方案，应选 WireMock/Hoverfly/MockServer，并可把 MSW 作为前端测试层补充。

### 与现有候选的补充核验（Mockoon）

Mockoon 官方 CLI 文档明确提供 headless CLI、Docker 镜像和 Admin API，可在运行中的环境导入/导出数据并控制 mock server：[CLI](https://mockoon.com/cli/)、[Admin API](https://mockoon.com/docs/latest/admin-api/overview/)。路由支持 Handlebars 模板、data buckets，以及动态规则和 JavaScript hooks；文件响应有专门的 file-serving 配置：[动态规则](https://mockoon.com/docs/latest/route-responses/dynamic-rules/)、[Data buckets](https://mockoon.com/docs/latest/data-buckets/overview)、[File serving](https://mockoon.com/docs/latest/response-configuration/file-serving/)。因此它可实现接口间共享数据，但需要确认具体版本的 hook API 是否开放文件系统/出站 HTTP；multipart 解析和持久化文件不是 WireMock 式一等 DSL，建议先做 PoC。项目采用 MIT 许可证：[GitHub LICENSE](https://github.com/mockoon/mockoon/blob/main/LICENSE.md)。

### 增补候选对比

| 新增/补充项目 | 动态配置 | 跨接口代码与外部调用 | multipart 上传 | 文件/二进制响应 | 许可证/部署 | 结论 |
|---|---|---|---|---|---|---|
| **Hoverfly** | REST simulation API 运行时替换 | Stateful pairs；外部 middleware 可调用存储/上游 | 无专用 part matcher，middleware 自行解析 | `encodedBody` + Base64；middleware 可读文件 | Apache-2.0；二进制、Docker | 录制回放和有限状态强；复杂文件流需 middleware |
| **MSW** | `server/worker.use`、`resetHandlers`（进程内） | JS resolver 共享 store、可 `fetch` 外部服务 | 原生 `Request.formData()`/`File`，代码自行校验 | `HttpResponse.arrayBuffer` 等 | MIT；npm，浏览器/Node | JS 测试内灵活；不是独立远程 mock 服务 |
| **Mockoon（补充）** | CLI/Admin API、GUI 热编辑 | Handlebars、data bucket、JS hooks | 依赖 hook 自行解析 | 模板/脚本设置 body 和头，版本相关 | MIT；桌面、CLI、Docker | 原型友好；复杂文件语义先验证版本 |

综合原有结论：独立服务且要求 multipart + 文件返回开箱即用时仍优先 WireMock；需要 Go 生态录制回放可评估 Hoverfly；纯 JS/TS 测试且强调每用例动态逻辑时可选 MSW。无论选型，都应在真实 multipart（大文件、重复字段）、并发状态隔离、进程重启和恶意文件名等场景做 PoC。

## 轻量方案调研（2026-09-12）

本节把“轻量”定义为：可以在一台开发机或 CI 中用单进程启动，依赖少，允许把配置/逻辑写入文件后重启生效；不再要求运行时通过 Admin API 动态增删接口。以下同时列出可直接使用的 Mock 产品和用于自建 Mock 的轻量框架。框架方案没有 GUI 或通用 Mock DSL，但能用普通代码精确实现“接口 A 取本地/外部数据，接口 B 读取 A 的结果”的逻辑。

### 最轻量：Go `net/http` 自建单二进制（框架/标准库，不是现成 Mock 产品）

Go 标准库的 [`net/http`](https://pkg.go.dev/net/http) 已包含路由所需的 `http.Handler`、`Request.ParseMultipartForm`、`http.ServeFile` 等 API。将路由和一个共享的 `Store` 结构体写在同一项目中，`go build` 生成单一可执行文件；修改 Go/JSON 配置后重启进程即可生效，不需要 JVM、Node 或 Python 运行时。Go 项目采用 BSD 风格许可证（[官方 LICENSE](https://go.dev/LICENSE)），自建 Mock 的业务代码可按团队许可证发布。

典型实现方式：

* `POST /upload` 先用 [`http.MaxBytesReader`](https://pkg.go.dev/net/http#MaxBytesReader) 限制大小，再调用 `ParseMultipartForm` 读取 `file` part；将内容写入临时目录或受控内存 Store，并生成业务 ID。
* `GET /metadata/:id` 在 `sync.RWMutex` 保护下读取 Store，仅返回上传文件的名称、大小或外部接口返回的字段。
* `GET /download/:id` 用 `http.ServeFile` 返回文件，或设置 `Content-Type`/`Content-Disposition` 后直接写入二进制；同一 Handler 可以用 `http.Client` 调上游并把结果缓存给后续接口。

这能完整覆盖上传、文件/二进制下载、跨接口状态和外部取数，运行开销最低；代价是需要自行编写路由、持久化、清理和测试。Store 必须按测试/租户 ID 隔离并加锁，文件名应使用随机 ID 或 `filepath.Base`，不能让请求路径直接决定磁盘路径。它适合接口数量有限但业务逻辑较复杂、希望 CI 只携带一个二进制的团队。

### Python FastAPI + Uvicorn（轻量自建服务）

[FastAPI](https://fastapi.tiangolo.com/) 是 MIT 许可（[仓库 LICENSE](https://github.com/fastapi/fastapi/blob/master/LICENSE)）的 Python Web 框架；[Uvicorn](https://www.uvicorn.org/) 是 ASGI 服务器（[BSD-3-Clause LICENSE](https://github.com/encode/uvicorn/blob/master/LICENSE.md)）。最小部署通常是一个 Python 环境加 `fastapi`、`uvicorn` 和处理 multipart 所需的 [`python-multipart`](https://github.com/Kludex/python-multipart)；以 `uvicorn app:app` 启动，改代码/配置后重启。

官方文档的 [`UploadFile`/`File`](https://fastapi.tiangolo.com/tutorial/request-files/) 支持 multipart 上传，[`FileResponse`](https://fastapi.tiangolo.com/advanced/custom-response/#fileresponse) 和 `StreamingResponse` 可返回文件或任意字节。路由函数可以共享一个进程内 Store，调用标准 `httpx`/`urllib` 请求外部服务，再让另一个路由读取 Store 中的字段。这样不需要引入专门的 Mock DSL，代码调试也直接；但 Python 解释器和依赖比 Go 单二进制更重，进程重启会丢失内存状态（要持久化需 SQLite/文件或外部存储）。生产部署应按 [Uvicorn deployment 文档](https://www.uvicorn.org/deployment/) 使用进程管理器，并设置上传大小、超时和临时目录配额。

### Python Flask + Werkzeug（轻量自建服务）

[Flask](https://flask.palletsprojects.com/) 采用 BSD-3-Clause（[仓库 LICENSE](https://github.com/pallets/flask/blob/main/LICENSE.txt)），可用一个 Python 脚本和 Flask 依赖启动。官方 [文件上传模式](https://flask.palletsprojects.com/en/stable/patterns/fileuploads/) 使用 `request.files`、`secure_filename` 和 `MAX_CONTENT_LENGTH`；[`send_file`](https://flask.palletsprojects.com/en/stable/api/#flask.send_file)/`send_from_directory` 可返回本地文件或二进制。普通路由函数可以在内存字典中保存接口 A 的结果，接口 B 再查询并拼装响应，也可以通过 `requests`/`httpx` 调上游。

Flask 自带开发服务器只适合测试；官方 [部署文档](https://flask.palletsprojects.com/en/stable/deploying/) 建议在需要时使用 Gunicorn、Waitress 等 WSGI 服务器。与 FastAPI 相比，Flask 的上传解析和响应 API 更朴素、依赖更少；代价是类型校验和异步支持需要自行选择。它是“几百行代码即可覆盖全部需求”的稳妥 Python 方案，而非开箱即用的 Mock 产品。

### Node Express + Multer（轻量自建服务）

[Express](https://expressjs.com/) 是 MIT 许可（[仓库 LICENSE](https://github.com/expressjs/express/blob/master/LICENSE)）的 Node HTTP 框架；官方 [`res.sendFile`](https://expressjs.com/en/api.html#res.sendFile) 可发送文件。multipart 解析可使用 Express 生态的 [`multer`](https://github.com/expressjs/multer#readme)，其 MIT 许可证和 `limits`、`fileFilter`、内存/磁盘存储选项均在仓库文档中说明。用一个 `server.js` 注册 `POST /upload`、`GET /metadata`、`GET /download`，共享 JS Store，并用 Node 内置 `fetch` 或 `https` 调外部服务，即可实现多接口编排；改 JSON/JS 配置后重启 Node 进程。

这套组合比 Java MockServer/WireMock 更易放进前端或 Node CI，但需要固定 Node 版本和 npm 依赖。必须配置 Multer 文件大小/数量限制，禁止把用户提供的文件名直接拼接到路径，并在下载响应中明确 `Content-Type`、`Content-Disposition`。Express 本身没有跨请求状态或文件持久化语义，相关逻辑都属于自建代码。

### json-server + 自定义 middleware（轻量 Mock 产品，适合 CRUD 为主）

[json-server](https://github.com/typicode/json-server) 是 MIT 许可（[LICENSE](https://github.com/typicode/json-server/blob/main/LICENSE)）的 Node 工具，`npx json-server db.json` 即可根据 JSON 文件提供 CRUD REST 接口。官方 README 的 [custom routes/middleware](https://github.com/typicode/json-server#custom-routes) 允许在生成的 Express server 上挂载自定义 Handler，因此可以把少量跨接口逻辑写在 `server.js`，修改 `db.json`/脚本后重启。

需要明确其边界：标准 json-server 没有文件上传解析、文件存储或二进制响应 DSL。可在自定义 middleware 中加入 Multer/Busboy，调用 `express.static`/`res.sendFile` 返回文件，并自行维护接口 A/B 之间的状态；这时运行时依赖已接近普通 Express 服务。若主要需求是快速得到 JSON CRUD、只有一两个自定义上传/下载接口，json-server 能减少样板代码；若文件和业务编排是核心，应直接使用 Go、FastAPI、Flask 或 Express 自建服务。

### httpbin（开源固定端点服务，不建议作为主 Mock）

[httpbin](https://github.com/postmanlabs/httpbin) 是基于 Flask 的 HTTP 请求/响应回显服务，仓库提供 [LICENSE](https://github.com/postmanlabs/httpbin/blob/master/LICENSE) 和 Docker/源码运行说明。现成端点可回显 POST 表单和文件（`/post`）、返回任意字节（`/bytes/:n` 等）或图片，适合验证客户端上传、下载、超时和错误处理。

它的端点集合由项目代码固定，不能仅靠 JSON 配置声明任意业务接口，也没有“接口 A 的结果供接口 B 查询”的持久状态模型。要实现这些逻辑必须 fork/扩展 Flask 应用并自行部署；因此应把 httpbin 当作通用协议测试辅助，而不是满足本需求的可配置 Mock 产品。采用前请以目标仓库当前 LICENSE 文件为准核对许可证（不同发行镜像可能来自不同维护仓库）。

### 轻量方案对比

| 方案 | 运行形态/依赖 | 跨接口代码与外部调用 | multipart 上传 | 文件/二进制返回 | 许可证 | 适配度 |
|---|---|---|---|---|---|---|
| **Go `net/http` 自建** | 单二进制、标准库 | Handler + 共享 Store + `http.Client`，完全可编程 | `ParseMultipartForm`（自行限流/校验） | `ServeFile` 或直接写 bytes | Go BSD；业务代码自定 | **最高**，最轻且能力完整，但需自己开发 |
| **FastAPI + Uvicorn** | Python + 少量 pip 包 | 路由函数/依赖注入，可调用上游 | `UploadFile`（需 `python-multipart`） | `FileResponse`/`StreamingResponse` | FastAPI MIT，Uvicorn BSD | **高**，开发效率好，依赖比 Go 多 |
| **Flask** | Python + Flask/WSGI | 普通函数和共享对象 | `request.files` | `send_file`/`send_from_directory` | BSD-3-Clause | **高**，API 简单稳定 |
| **Express + Multer** | Node + npm 包 | JS Handler/Store + `fetch` | Multer | `res.sendFile`/Buffer | Express/Multer MIT | **高**，适合 JS 团队 |
| **json-server + middleware** | Node + json-server | 自定义 middleware 可编程 | 需额外 Multer/Busboy | 需自定义静态/文件 Handler | MIT | **中**，CRUD 快；复杂文件逻辑不再轻 |
| **httpbin** | Flask 服务 | 固定端点；扩展需 fork | `/post` 可回显，非业务存储 | `/bytes`、图片等固定端点 | 以仓库 LICENSE 为准 | **低**，只适合协议回显测试 |

### 针对本需求的轻量落地建议

1. **首选 Go `net/http` 自建**：当目标是单一可执行文件、配置修改后重启、且接口 A/B 有真实业务编排时，它同时满足上传、下载、二进制和外部调用，运行资源最少。
2. **已有 Python 团队选 FastAPI 或 Flask**：FastAPI 适合类型校验和异步上游调用；Flask 适合极简同步脚本。将状态抽象为可替换 Store（内存/SQLite），测试结束清理临时文件。
3. **已有 Node/前端团队选 Express + Multer**；只有大量简单 CRUD 时才在其上使用 json-server，再通过 middleware 添加少量业务接口。
4. **把 httpbin 限定为辅助容器**，用于检查上传、下载、压缩、超时等客户端行为，不要把它当作主业务 Mock。

无论框架还是产品，都要在 CI 中固定版本并做四步 PoC：上传大文件（含重复字段）→返回业务 ID→查询接口读取部分字段→下载并校验二进制 checksum。并发运行时按测试 ID 隔离 Store，重启后明确接受状态丢失或启用 SQLite/文件持久化；对上传文件名、路径、大小、压缩炸弹和外部 URL 做白名单与资源限制。

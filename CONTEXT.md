# Stunt Double

Stunt Double 是面向真实外部依赖集成测试的 mock server。本词汇表定义项目语言，不含任何实现细节。

## Language

**Route**：
一个声明的 mock endpoint，包含匹配条件与一条响应流水线。
_Avoid_: endpoint definition, handler, stub

**Match**：
Route 中判断某个入站请求是否属于它的部分。
_Avoid_: filter, predicate, selector

**Source**：
构建 Response 所用数据的来源，例如静态文件或上游 HTTP 调用。
_Avoid_: data provider, backend, fetcher

**Transform**：
把 Source 数据转换为响应数据的脚本步骤。
_Avoid_: mapper, converter, processor

**Response**：
命中 Route 后返回给客户端的状态、header 与 body。
_Avoid_: reply, output, result

**Host function**：
由 Rust 宿主注入脚本运行时的函数。脚本的全部外部能力都来自 host function。
_Avoid_: native function, binding, built-in

**Script sandbox**：
约束脚本执行的限制与能力集合。
_Avoid_: jail, container, isolation layer

**Static file root**：
脚本可读取文件的唯一配置目录。
_Avoid_: file root, document root, upload directory

**Upstream failure**：
未能从上游调用取得 HTTP 响应。非 2xx 的 HTTP 响应是数据，不是 upstream failure。
例外：把 body 直接交给客户端的流式能力无法把最终非 2xx 响应当作数据；它把该响应表现为可捕获错误，使脚本仍能掌握客户端可见的状态。
_Avoid_: upstream error, backend error

**Request-local state**：
在单个请求期间存放、请求结束即销毁的数据。
_Avoid_: session, cache, shared state

**Shared state**：
跨请求持久存在或可见的数据。Stunt Double v1 不提供。
_Avoid_: global state, cross-request state

**Route model**：
每个接口都显式声明的配置模型。
_Avoid_: resource model, auto CRUD

**Resource model**：
由数据文件的形状派生 REST 路由的配置模型。
_Avoid_: route model, json-server mode

**Preset generator**：
把 OpenAPI 文档等上层输入编译为普通 Route 的工具。
_Avoid_: resource engine, second engine

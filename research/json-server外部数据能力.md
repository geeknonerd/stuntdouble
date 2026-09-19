# json-server 外部数据 Mock 能力调研

调研对象：[`typicode/json-server`](https://github.com/typicode/json-server)。结论针对仓库当前主线文档所描述的 JavaScript/Express 用法；运行前应以项目实际安装的版本（`npm ls json-server`）核对导出 API，因为 v0.x/v0.17 与 v1 的导入方式有差异。

> 核验说明：当前执行环境无法访问 GitHub/NPM 网络，因此本文引用的是 json-server 官方仓库、源码和发布页的固定链接，未在本次运行中重新抓取页面内容。落地前请打开对应链接并按实际安装版本核对 API 签名。

## 结论摘要

| 需求 | 是否可实现 | 说明 |
| --- | --- | --- |
| Mock 接口从外部 HTTP 接口获取 JSON，处理后返回 | 可以，但属于自定义路由/Express 中间件代码，不是 `json-server db.json` CLI 的内建数据源功能 | 在 json-server 创建的 Express 应用上注册自己的 `app.get/post(...)` 或 middleware，在处理器中调用 Node 的 `fetch`（或其他 HTTP 客户端），转换结果后用 `res.json(...)` 返回。 |
| Mock 接口从外部网络获取文件再返回 | 可以，但同样需要自定义处理器 | 处理器中获取 `ArrayBuffer`/流，设置正确的 `Content-Type`、`Content-Length`/缓存头，再用 `res.send(Buffer)` 或流式 `pipe` 返回；不要把二进制文件放进 JSON Server 的 JSON 数据库路由。 |
| 仅通过 CLI 参数配置上述两类代理/转换 | 不可以（官方 README 没有提供此能力） | CLI 主要负责把 JSON/JSON5/JS 数据文件暴露为 REST 资源；外部抓取和任意转换需写 JavaScript 扩展。 |

## 官方依据

1. **定位与 CLI 行为**：项目 README 将 json-server 定义为“用一个 JSON 文件快速启动 REST API”，示例为 `npx json-server db.json`，并展示资源路由、过滤、分页、排序等由本地数据文件驱动的行为。

   来源：<https://github.com/typicode/json-server#json-server>

2. **自定义路由**：README 的 *Custom routes* 部分说明可以提供 `routes.json`，把自定义路径重写到资源路径。这是 URL 重写，不是从远程 HTTP 服务读取和转换响应。

   来源：<https://github.com/typicode/json-server#custom-routes>

3. **自定义 middleware/JavaScript API**：README 的 *Module*（或 *Using as a module*）部分展示通过 JavaScript 创建 server、挂载 `defaults()` 与 `router(...)`，然后 `server.use(...)`/注册 Express 路由。由于返回的对象是 Express 应用，Express 支持的请求处理器（调用 `fetch`、设置响应头、`res.json`、`res.send`、流式响应等）都可以放在 json-server 路由之前或之后。

   来源：<https://github.com/typicode/json-server#module>

4. **源码结构**：官方仓库把 HTTP 应用、路由器和默认 middleware 分开导出；可从 `src/app.ts`、`src/router.ts`、`src/service.ts`（文件名可能随主线版本调整）核对 `create`/`router`/`defaults` 的实现与导出。仓库源码是判断可扩展边界的最终依据。

   来源：<https://github.com/typicode/json-server/tree/main/src>

5. **历史版本提示**：npm 上的 0.x/0.17 文档常见 `import jsonServer from 'json-server'; const server = jsonServer.create(); const router = jsonServer.router('db.json')` 写法；主线新版本可能改为命名导出。不要跨版本直接复制导入语句。

   来源：<https://github.com/typicode/json-server/releases> 、<https://www.npmjs.com/package/json-server>

## 推荐实现

以下示例展示“外部 JSON → 处理 → Mock 响应”。下面采用官方历史文档中最常见的 default import 写法；如果安装的主线版本改用命名导出，请以该版本 README/类型声明为准，仅调整导入语句，路由思路不变：

```js
import jsonServer from 'json-server'

const app = jsonServer.create()
app.use(jsonServer.defaults())

app.get('/mock/users', async (req, res, next) => {
  try {
    const upstream = await fetch('https://api.example.com/users')
    if (!upstream.ok) {
      return res.status(upstream.status).json({ error: 'upstream_failed' })
    }
    const body = await upstream.json()
    const users = body.items.map(({ id, name }) => ({ id, name }))
    return res.json({ users })
  } catch (error) {
    return next(error)
  }
})

app.use(jsonServer.router('db.json')) // 其他资源仍由 json-server 提供
app.listen(3000)
```

“外部文件 → Mock 文件响应”可以这样实现：

```js
app.get('/mock/file', async (req, res, next) => {
  try {
    const upstream = await fetch('https://files.example.com/report.pdf')
    if (!upstream.ok) return res.sendStatus(upstream.status)

    const contentType = upstream.headers.get('content-type') || 'application/octet-stream'
    const bytes = Buffer.from(await upstream.arrayBuffer())
    res.set('Content-Type', contentType)
    res.set('Content-Length', String(bytes.length))
    return res.send(bytes)
  } catch (error) {
    return next(error)
  }
})
```

大文件应优先采用上游响应体的流式转发（Node/Express 版本支持时），避免 `arrayBuffer()` 将整个文件读入内存；同时透传或白名单化 `Content-Disposition`、缓存和范围请求（`Range`）等头。

## 工程与安全注意事项

- **超时、重试和错误映射**：为上游请求设置 AbortController 超时；区分上游 4xx/5xx、网络错误和本地处理错误，避免所有错误都返回 200。
- **SSRF 防护**：不要直接把用户提供的 URL 交给服务器抓取。限制协议（仅 `https`）、域名白名单、重定向次数和响应大小，过滤内网/metadata IP。
- **响应头安全**：仅透传允许的响应头；避免把上游的 `Set-Cookie`、CORS、跳转等头无条件转给 Mock 客户端。
- **缓存与可重复性**：Mock 通常要求稳定结果。可在 middleware 中缓存上游结果，或在测试环境固定 fixture；记录上游版本/时间戳以便复现。
- **CORS 与鉴权**：浏览器访问 Mock 时的 CORS 由本地 Express middleware 配置；上游鉴权凭据应放在服务器环境变量，不能暴露给浏览器。
- **路由顺序**：自定义路由应在 `app.use(router(...))` 之前注册，否则可能先被 JSON Server 的通用资源路由处理；用明确前缀（如 `/mock/`）避免与数据库资源冲突。

## 最终判断

json-server 可以作为“带 REST 资源的 Express Mock 外壳”：通过 JavaScript API 注册自定义处理器后，既能调用外部接口并加工 JSON，也能下载并返回外部二进制文件。它本身不是通用反向代理，CLI 单独运行时没有“远程数据源 + 转换脚本 + 文件透传”配置；若需求包含复杂代理、鉴权、缓存、断点续传或高并发文件流，建议直接使用 Express/Fastify 服务，或在 json-server 之外增加专门代理层。

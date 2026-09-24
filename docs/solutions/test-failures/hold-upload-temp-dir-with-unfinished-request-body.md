---
title: "Hold a transient upload temp directory with an unfinished request body"
date: 2026-09-24
category: test-failures
module: upload temp directory integration test
problem_type: test_failure
component: testing_framework
symptoms:
  - "The required `test` job intermittently panics in `upload_client_filename_is_never_used_on_disk` with `upload temp directory never appeared under ...`."
  - "Two of 12 CI runs failed on 2026-09-24 on unrelated commits while the other 148 tests passed."
  - "The same test passes on most reruns, so the failure depends on CI scheduling rather than a stable repository breakage."
root_cause: async_timing
resolution_type: test_fix
severity: medium
related_components:
  - server
  - files
  - tests
tags: [flaky-test, integration-test, async-timing, request-scoped-temp-dir, multipart-upload, deterministic-synchronization, upload-cleanup, ci]
---

# 上传临时目录测试：用未完成的请求体持有状态，而不是 sleep 抢窗口

## 问题

`tests/cli.rs` 的 `upload_client_filename_is_never_used_on_disk` 在 CI 上间歇性失败。`test` 是必需检查，每次出现都要重跑 job 才能继续合 PR：2026-09-24 同一条 panic 出现两次（issue #76 记录：当天 12 次 CI 运行中两次失败，一次在 main 推送、一次在 docs-only PR），两次的运行输出里都只有这一个测试失败（`148 passed; 1 failed`）。要证明的安全属性——客户端提供的文件名永远不落盘——并没有问题，坏的是它的观察方式：断言盯着的「请求级临时目录」在请求结束时会被清理，于是断言变成和清理赛跑。

## 症状

- 同一条 panic 文本间歇出现：`panicked at tests/cli.rs:566:9: upload temp directory never appeared under /tmp/.tmpXXXX/uploads-tmp`（修复前行号；对应断言现在位于 `tests/cli.rs:569-573`）。
- 与改动无关：两次失败分别落在两个互不相关的提交上，其中一次是只改文档的 PR。
- 失败信息本身不足以定位：issue #76 已写明，「目录在窗口内不可见」无法区分「从未创建」「已被删除」「建在别处」，所以根因不能从 panic 文本直接读出来。
- 本地不复现：修复前连续 33 次本地运行全绿，本地重复不能作为「不存在竞态」的证据。

## 试过但无效的做法

- **旧设计：客户端线程 + mpsc 握手 + 2 秒 sleep。** 客户端线程写完整 1 MiB multipart body，读到响应体开始后经 channel 通知主线程，再 sleep 2 秒，注释写着 `Keep the response open so the server-side relay blocks and the request-scoped directory stays observable`。这条注释是错的：1 MiB 响应体能整段进入内核 socket 缓冲，服务器不必等客户端读取就能结束中继并释放 `UploadStore`，目录在客户端还在睡时就被删掉。给 `UploadStore` 的创建与 drop 打时间戳后测得：目录只存活 53.2–53.7 ms，而测试的第一次观察发生在客户端首次读到响应之后的 50–630 µs——约 50 ms 的余量，负载高的 runner 会输掉这场赛跑。
- **只重跑失败的 job。** 重跑确实让 PR 继续推进，也确认这次失败可恢复（因而可以判断它不是稳定破坏），但它不产生任何根因信息；本次是在重跑通过之后仍然按 flake 立案（issue #76），再靠插桩定位。
- **放宽轮询超时或加长 sleep。** 单纯放宽超时只是掩盖同一个竞态：窗口一旦被拖长，竞态依旧，只是更晚暴露。
- **靠本地重复运行验证修复。** 33 次本地 pre-fix 运行没有一次复现；这里唯一能给出答案的是测量（插桩时间戳），不是重复次数。

## 解决方案

修复是 PR #77（2026-09-24 合并到 main），只动 `tests/cli.rs`，生产代码不变：

1. **客户端只发一半请求体，然后握着不放。** 写请求头加 multipart body 的开头（file part 头 + 4096 字节数据），不再靠响应阶段决定观察时机（`tests/cli.rs:2453-2473`）。切分点是 part 头之后第一个 `\r\n\r\n` 再加 4 字节，保证解析器能走到「创建文件」这一步（`tests/cli.rs:2453-2459`）。
2. **把观察窗口挪进解析阶段。** `parse_multipart()` 在读 body 之前就调用 `create_upload_dir()`（`src/files.rs:325`），随后在 `field.chunk()` 循环里逐块落盘（`src/files.rs:356-373`）；请求体读不完它就不会返回，所以 body 未完成期间目录的存在由解析器自己持有，而不是由定时器保证。
3. **等待助手只用于 fixture 主动持有的状态。** `wait_for_temp_entries` 改名为 `wait_for_entries(root, what)`（`tests/cli.rs:562-576`），同一套收敛等待同时覆盖目录和目录内的文件；它的 doc comment 就是这条规则的落点：`Only use this for a state the fixture holds open (an unfinished request body, for example); a state that can come and go needs a synchronization point instead.`（`tests/cli.rs:559-561`）
4. **安全断言全部保留。** 等待到的目录必须恰好一个；Unix 上 mode 必须是 `0o700`（`tests/cli.rs:2476-2487`）；目录内文件名必须是不透明数字串，且不等于 `forbidden-client-name.bin`（`tests/cli.rs:2488-2499`）；stderr 不得出现客户端文件名或临时根路径（`tests/cli.rs:2516-2523`）。
5. **观察完仍走正常路径。** 断言之后补发 body 剩余部分，脚本 `ctx.respond(200, {}, ctx.request.files[0].stream())` 流式返回文件，断言 200，再用 `wait_for_empty_temp` 确认清理（`tests/cli.rs:2503-2513`）——清理断言本身是收敛的，不受本次改动影响。
6. **删掉同步残骸。** 客户端线程、mpsc 握手、2 秒 sleep 全部删除；测试从 2.10 s 降到 0.13 s。

## 为什么这样可行

根因是所有权，不是时序运气。目录的生命周期如下：

- `create_upload_dir()` 返回 `TempDir`，Unix 下构造时把权限固定成 `0o700`（`src/files.rs:406-413`）。
- 解析期间它先是 `parse_multipart()` 的局部变量；解析成功后才移进 `UploadStore { dir: Mutex<Option<TempDir>>, .. }`，而该类型的注释写明它就是「在最后一个持有者 drop 时删除随机临时目录」的守卫（`src/files.rs:191-198`、`src/files.rs:397-401`）。
- 脚本用 `ctx.request.files[0].stream()` 返回文件时，流式响应的 `FileBody` 持有该守卫（`src/files.rs:944-946`），中继函数把它留在函数体内直到返回（`src/server.rs:941-943`）。但这只把生命周期延长到「中继结束」，而中继结束并不要求客户端读过数据：本次实测中 1 MiB 响应在约 53 ms 内就写完了发送路径（`src/server.rs:957-986`），函数返回守卫释放，目录随即被删除。客户端的 sleep 从来没有让服务器卡住。

所以旧测试观察的「响应阶段」是典型的会来又会走的状态；新测试改成观察「解析阶段」：要删掉目录，必须等 `parse_multipart()` 把请求体读完，而请求体的最后一段正握在测试自己手里。`wait_for_entries` 等的事件由测试自身的动作驱动，必然发生，不再需要猜时机。

## 预防

- **不要断言一个会来又会走的状态。** 观察到它只说明「某个瞬间它在」；观察不到既不能证明它从未存在，也不能证明它被正确清理。这类断言只有两种合法写法：让 fixture 主动持有该状态（本次做法），或断言一个收敛状态（例如清理完成后的空目录，`wait_for_empty_temp`）。
- **未完成的请求体是把请求级状态钉住的最省事手段。** 它零依赖：不需要探针接口、专用日志或测试后门，直接用客户端握住的连接即可；这条规则已经写进 `wait_for_entries` 的 doc comment（`tests/cli.rs:559-561`）。
- **注释也按断言审查。** 旧注释宣称 sleep 能让服务端 relay 阻塞，从未被验证，且被测量证伪。凡是「我让服务器忙起来了」式的假设，要么用测量支撑（本次是给 `UploadStore` 的创建与 drop 打时间戳），要么删掉。
- **本地重复不是证据。** 33 次本地运行全绿，缺陷依然存在；间歇性问题要靠插桩/测量定位，「再跑一遍看看」只会延长定位时间。
- **注意与请求读取期限的耦合。** 保持 body 不完成的观察窗口（本例等待上限 5 s，`tests/cli.rs:563`）必须小于 `server.request_timeout_ms` 的默认值（`src/config.rs:682-684`）；将来若缩短该期限，测试会以「服务器提前结束请求」的形式确定性失败，而不是退回竞态。
- **行业实践同向。** Martin Fowler 的 [Eradicating Non-Determinism in Tests](https://martinfowler.com/articles/nonDeterminism.html) 明确反对用 sleep 等待异步状态，要求把异步边界变成可观察事件；Google 的 [flaky 测试统计](https://testing.googleblog.com/2017/04/where-do-our-flaky-tests-come-from.html) 也表明这类失败应按缺陷调查，而不是当作噪声重跑。

## 相关

- GitHub issue #76：flake 报告，含两次 CI run 与「加长超时只是掩盖竞态」的判断；PR #77：修复并合并（2026-09-24），提交信息记录了 50 次连续运行、全量本地门禁与 CI 转绿。
- 回归测试：`tests/cli.rs:2418-2524`（本测试）、`tests/cli.rs:559-591`（`wait_for_entries` 与 `wait_for_empty_temp`）。
- 同一根因的另一半（响应完成不等于客户端读完）：[blocking-upstream-body-to-async-stream-bridge.md](../architecture-patterns/blocking-upstream-body-to-async-stream-bridge.md)。
- 同类「用固定 sleep 代替同步」的既有教训：[serve-shutdown-second-signal-semantics.md](../conventions/serve-shutdown-second-signal-semantics.md)。
- 相邻的请求体截止与 multipart 清理边界：[request-read-deadline-at-the-connection-layer.md](../conventions/request-read-deadline-at-the-connection-layer.md)、[separate-multipart-data-budget-from-raw-body-limit.md](../conventions/separate-multipart-data-budget-from-raw-body-limit.md)、[host-boundary-fail-closed-input-classification.md](../conventions/host-boundary-fail-closed-input-classification.md)。
- 更早的 E2E 稳定化先例：issue #35 与 PR #38（把端口启动竞态换成可观察的就绪证据）。

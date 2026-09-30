# Security Policy

## Supported versions

| Version | Supported |
| --- | --- |
| 0.5.x | Supported (pre-1.0 line) |
| < 0.5.0 | Not supported |

Stunt Double is pre-1.0 (latest `v0.5.3`), with slices T1–T13 implemented. Security fixes ship in the next patch release on the `0.5.x` line. Once `1.0.0` exists, this table lists the supported stable lines.

## Threat model

Stunt Double runs the route scripts you supply; it is not a sandbox for untrusted code. Scripts are semi-trusted input. The host blocks raw network, file, process, and socket access and enforces a response deadline, a bounded fail-fast script-worker pool, plus loop-iteration, recursion, and VM-stack limits. On Linux each worker applies `sandbox.script_memory_limit_mb` (default 256 MiB, minimum 64 MiB) as `RLIMIT_AS` before script code runs; this bounds virtual address space, not RSS. Boa 0.22 exposes no heap metric, heap limit, or interrupt hook. macOS cannot enforce a useful `RLIMIT_AS` bound in this slice, so macOS and Windows retain process isolation and deadline kill without a hard memory bound; the host enforces the deadline by killing the worker process. Run the server only where the clients and script authors are trusted, and keep the default `127.0.0.1` bind unless another boundary sits in front of it. See [ADR 0014](plans/adr/0014-process-isolated-script-runner.md) and the T3 and #29 amendments in [ADR 0003](plans/adr/0003-script-first-multi-runtime.md) for the full tradeoff and the remaining resource bounds.

## Script worker isolation

Every matched Route runs its script in a fresh worker process spawned from the same binary through a hidden subcommand. One request is one worker: there is no process reuse and no cross-request state. Every host capability stays in the parent process — the upstream allowlist and HTTP clients, the static file root, multipart storage, Range and stream framing, and response-header validation. The worker runs only Boa and requests those capabilities over a strict JSON Lines protocol on stdin/stdout; stdout carries protocol messages only, while stderr stays operator diagnostics. A crash, protocol failure, or deadline therefore fails that request with `500 script_error` and cannot take down another route; the parent kills and reaps the worker before releasing its slot.

Worker lifetime is bounded on every path. The configured `sandbox.script_timeout_ms` covers spawn, script execution, host-call round trips, and delivery of the final result. On expiry, the parent kills the worker (`SIGKILL` on Unix, `TerminateProcess` on Windows) and drops any parent-held pipe or file streams, so a client never sees a partial 200 body from a timed-out script. Normal shutdown stops accepting new connections, drains in-flight requests within their own deadlines, and kills any worker still alive at the end of draining.

A hard kill of the server (`SIGKILL`) cannot run that cleanup. On Unix, each worker runs a small sentinel thread that polls `parent_id()` once per second and exits when it observes reparenting; the sentinel is independent of the thread running the script, so a runaway script cannot suppress it. Windows has no equivalent yet: drain-time kill still applies, but hard-kill cleanup and a hard memory bound are deferred to [#102](https://github.com/geeknonerd/stuntdouble/issues/102) with the Job Object work. The Unix sentinel uses only standard-library `parent_id()` polling; `PR_SET_PDEATHSIG` and process groups are not used.

On Linux the worker applies `sandbox.script_memory_limit_mb` (default 256 MiB, minimum 64 MiB) as `RLIMIT_AS` (soft = hard) before it reads the job, so job deserialization, IPC buffers, buffered file reads, Boa, and script code all count against the bound. It bounds virtual address space, not resident set size. macOS cannot establish a useful `RLIMIT_AS` bound in this slice (#89); macOS and Windows keep process isolation and the deadline but have no hard memory bound. The final Linux budget measurement and platform decision are recorded in [ADR 0014](plans/adr/0014-process-isolated-script-runner.md).

## Static file access

`ctx.file` reads resolve against the single configured static file root. Absolute paths and any `..` component are rejected before resolution, and the canonicalized target must stay inside the canonicalized root, so a symlink cannot widen the readable set; the root itself gets the same check. Buffered reads (`readText`, `readBytes`) are capped at 8 MiB; `stream` bypasses the size cap by design but stays confined to the same root. File paths and file names never enter logs.

Resolution and open are separate operations. On Linux the opened descriptor's target is re-checked through `/proc/self/fd` before any byte is read, which narrows the race; other platforms keep the pre-open check only. A local writer who can rewrite directory entries inside the root can in principle still race that window, which is an accepted tradeoff for the local, semi-trusted-script model. The upgrade path is `openat`-style resolution with `O_NOFOLLOW` (and equivalent platform APIs) when the root must also be defended against untrusted local writers.

## Multipart uploads

Multipart parsing runs before the script only for a matched Route. File parts are streamed into a random directory under the system temporary directory, created with owner-only (`0700`) permissions on Unix; non-file field data is counted but discarded. A part without a `name` attribute is rejected as malformed, and files use opaque numbered names, never client-provided filenames. `files.upload_max_bytes` (default 20 MiB) is enforced by a `Content-Length` pre-check plus streaming accounting that includes non-file fields. The entire multipart body stream, including framing, is bounded by `files.upload_max_bytes` plus a 1 MiB framing allowance; the data budget itself excludes framing, and streaming accounting remains authoritative. Buffered upload reads (`text()`, `bytes()`) are capped at 8 MiB; `stream()` is uncapped but is relayed to the client only as a `ctx.respond` body under the same Range and host-owned framing rules as local file streams.

The temporary-directory guard removes upload storage on buffered responses, script errors, parse errors, timeouts, stream completion, and client disconnect. If the script deadline expires while the blocking worker is still running, the host deletes the contents immediately and keeps the guard as a retry if the platform still blocks deletion. Client filenames are never used as filesystem paths and never written to logs; only the basename reaches `ctx.request.files[].filename`. Temporary upload paths are never exposed or logged.

## Request read deadlines

`serve` reads one request head and body under `server.request_timeout_ms` (default 30 s, positive integer). A head that does not complete in that window closes the connection and records the log-only class `request_head_timeout`; a body or multipart parse that does not finish answers 408 `request_timeout` and drops the request-scoped upload directory, because the parse future owns the temporary-directory guard. The deadline is independent of `sandbox.script_timeout_ms`, which still bounds script execution only.

## Reporting a vulnerability

Do not open a public issue for a security vulnerability. Use GitHub private vulnerability reporting for this repository:

https://github.com/geeknonerd/stuntdouble/security/advisories/new

If private reporting is temporarily unavailable, open a minimal public issue that asks for a private reporting channel. Do not include exploit details, customer data, tokens, hostnames, logs, or proof-of-concept payloads in that issue.

Include the following in a private report when possible:

- A short description of the issue and affected area.
- A minimal reproduction or proof of concept.
- The impact you believe is possible.
- Any suggested mitigation or fix.
- Whether you want public credit after disclosure.

## Security-relevant areas

This project intentionally embeds script runtimes and talks to external systems. Reports are especially useful in these areas:

- Script sandbox escapes or host-function bypasses.
- SSRF or allowlist bypass through upstream HTTP APIs.
- Path traversal or symlink escape around the static file root.
- Upload handling bugs, temporary file leaks, or size-limit bypasses.
- Response splitting, header injection, or Range handling bugs.
- Secret leakage through logs, diagnostics, or error responses.
- Denial of service through script timeouts, memory limits, or stream handling.

## Disclosure expectations

We will acknowledge a report within 48 hours, investigate it, and coordinate a disclosure timeline with the reporter. Please give us reasonable time to ship a fix before publishing details.

Security fixes ship in the next patch release. If the issue is critical, an emergency pre-release may be published.

## Privacy rule

Never include customer hostnames, internal URLs, credentials, API keys, production logs, request bodies, or response bodies in a security report. Redact or replace them with public demo values from `plans/demo-document-catalog.md`.

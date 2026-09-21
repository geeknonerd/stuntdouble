---
title: Stunt Double
---

# Stunt Double

**A test double that plays the whole show.**

Stunt Double is a Rust-based mock server for integration testing against real external dependencies. It reads upstream APIs, transforms data with built-in JavaScript, returns files and binary responses, and runs without host runtime dependencies.

## Status

Slices T1–T6 are implemented. `stuntdouble serve` and `stuntdouble validate` run from a TOML configuration; matched routes execute JavaScript in the embedded Boa runtime with a host-injected `ctx` (`apiVersion`, `request`, `http.get`, `http.pipe`, `respond`, `log`, `env`); `ctx.http.get` performs allowlisted upstream HTTP calls, and `ctx.http.pipe` streams an allowlisted upstream body to the client with Range passthrough. Unmatched routes return 404 `not_found`; script failures return 500 `script_error` or `script_no_response`; uncaught upstream transport failures return 502 `upstream_unreachable`. The document manifest and PDF download demo scenarios run from the in-repo fixture in `demo/`. Local static files, uploads, and file responses are the next slices.

## Documentation

- [README](https://github.com/geeknonerd/stuntdouble/blob/main/README.md) — project overview and current status.
- [Public contracts](https://github.com/geeknonerd/stuntdouble/tree/main/docs/contracts) — configuration, `ctx` API, and CLI.
- [Demo fixture](https://github.com/geeknonerd/stuntdouble/blob/main/demo/README.md) — runnable manifest and PDF download scenario.
- [`ctx` API type definitions](https://github.com/geeknonerd/stuntdouble/blob/main/types/ctx-api-v1.d.ts) — `apiVersion` 1 source types.
- [Changelog](https://github.com/geeknonerd/stuntdouble/blob/main/CHANGELOG.md) — release history.
- [Contributing](https://github.com/geeknonerd/stuntdouble/blob/main/CONTRIBUTING.md)
- [Security policy](https://github.com/geeknonerd/stuntdouble/blob/main/SECURITY.md)

## License

Dual-licensed under MIT OR Apache-2.0, at your option.

---
title: Stunt Double
---

# Stunt Double

**A test double that plays the whole show.**

Stunt Double is a Rust-based mock server for integration testing against real external dependencies. It reads upstream APIs, transforms data with built-in JavaScript, returns files and binary responses, and runs without host runtime dependencies.

## Status

Slices T1–T5 are implemented. `stuntdouble serve` and `stuntdouble validate` run from a TOML configuration; matched routes execute JavaScript in the embedded Boa runtime with a host-injected `ctx` (`apiVersion`, `request`, `http.get`, `respond`, `log`, `env`); and `ctx.http.get` performs allowlisted upstream HTTP calls. Unmatched routes return 404 `not_found`; script failures return 500 `script_error` or `script_no_response`; uncaught upstream transport failures return 502 `upstream_unreachable`. The document manifest demo scenario runs from the in-repo fixture in `demo/`. File and binary responses are the next slices.

## Documentation

- [README](https://github.com/geeknonerd/stuntdouble/blob/main/README.md)
- [Documentation index](https://github.com/geeknonerd/stuntdouble/tree/main/docs)
- [Product definition](https://github.com/geeknonerd/stuntdouble/blob/main/plans/product-definition.md)
- [Demo fixture](https://github.com/geeknonerd/stuntdouble/blob/main/demo/README.md)
- [Public contracts](https://github.com/geeknonerd/stuntdouble/tree/main/docs/contracts)
- [ctx API type definitions](https://github.com/geeknonerd/stuntdouble/blob/main/types/ctx-api-v1.d.ts)
- [Architecture decisions](https://github.com/geeknonerd/stuntdouble/tree/main/plans/adr)
- [Research notes](https://github.com/geeknonerd/stuntdouble/tree/main/research)
- [Contributing](https://github.com/geeknonerd/stuntdouble/blob/main/CONTRIBUTING.md)
- [Security policy](https://github.com/geeknonerd/stuntdouble/blob/main/SECURITY.md)

## License

Dual-licensed under MIT OR Apache-2.0, at your option.

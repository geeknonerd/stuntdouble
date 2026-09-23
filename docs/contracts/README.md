# Public contracts

**English** \| [中文](./README.zh-CN.md)

Stunt Double exposes three public contracts. They evolve separately and are tracked here.

| Contract | Status | Applies to | Stability |
| --- | --- | --- | --- |
| [Configuration](config.md) | frozen for the v1 slice; additive updates through T12 | v0.x | Breaking changes allowed before 1.0 with a deprecation window |
| [`ctx` API](ctx-api.md) | public for the v1 slice; implemented subset through T12 | `apiVersion` 1 | Additive within an `apiVersion`; removals require a new version |
| [CLI](cli.md) | frozen for the v1 slice; additive updates through T12 | v0.x | Breaking changes allowed before 1.0 with a deprecation window |

## Change process

1. Update the relevant contract file in the same pull request as the implementation.
2. Update `CHANGELOG.md` with the user-visible change and migration note.
3. Warn for at least one minor version before removing or renaming a public surface.
4. Record a hard-to-reverse compatibility decision as an ADR under `plans/adr/`.
5. For the `ctx` API, update the matching `.d.ts` file when the contract changes. The apiVersion 1 source definition lives at [`types/ctx-api-v1.d.ts`](../../types/ctx-api-v1.d.ts) and is required in the release artifact set by [ADR 0012](../../plans/adr/0012-release-artifacts-and-supply-chain.md).

## Versioning rules

- Configuration and CLI follow the product version until 1.0.
- The `ctx` API uses its own `apiVersion` and may outlive a product major version.
- `1.0.0` requires a stable configuration format, CLI, and `ctx` API version 1.
- Details are in [ADR 0011](../../plans/adr/0011-contract-compatibility.md).

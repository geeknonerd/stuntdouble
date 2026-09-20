# Documentation

This directory holds the operational, contractual, and agent-facing documentation for Stunt Double.

## Start here

- [Development and release workflow](development.md) — Git workflow, CI, versioning, release process, MSRV, and dependency policy.
- [Public contracts](contracts/) — configuration, `ctx` API, and CLI contracts.
- [Agent documentation](agents/) — issue tracker, triage labels, and domain-doc consumption rules.
- [GitHub Pages landing page](index.md) — public entry page.

## Other documentation

- [README](../README.md) — project overview and status.
- [Chinese README](../README.zh-CN.md) — 中文项目说明.
- [Product definition](../plans/product-definition.md) — v1 scope and non-goals.
- [Architecture decisions](../plans/adr/) — ADR 0001–0012.
- [Research](../research/) — mock server landscape, runtime selection, and open-source baseline.
- [Solutions](solutions/) — learnings and resolved problems via ce-compound.
- [ctx API type definitions](../types/ctx-api-v1.d.ts) — `apiVersion` 1 source types; T8 publishes them with release artifacts.

- [Governance](../GOVERNANCE.md) — maintainer model and response expectations.
- [Security policy](../SECURITY.md) — vulnerability reporting and security expectations.
- [Changelog](../CHANGELOG.md) — release history.
- [Agent instructions](../AGENTS.md) — repository rules for coding agents.

## Documentation rules

- Root community docs are written in English.
- Design docs and research notes may be written in Chinese; English translations are welcome.
- Public contract changes update `docs/contracts/` and `CHANGELOG.md`.
- Domain terminology changes update `CONTEXT.md`.
- Hard-to-reverse decisions become ADRs under `plans/adr/`.

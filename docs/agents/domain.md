# Domain Docs

How the engineering skills should consume this repo's domain documentation when exploring the codebase.

## Before exploring, read these

- **`CONTEXT.md`** at the repo root — the project glossary.
- **`plans/adr/`** — read ADRs that touch the area you're about to work in. This repo keeps ADRs there instead of `docs/adr/`.
- **`plans/product-definition.md`** — v1 scope, host API contract, and the do-not-build list.
- **`plans/demo-document-catalog.md`** — the public demo scenario for CSV manifest generation and binary download.
- **`docs/contracts/`** — configuration, `ctx` API, and CLI contracts.
- **`docs/development.md`** — Git workflow, CI, versioning, and release rules.

If a file doesn't exist, **proceed silently**. Don't flag its absence. `CONTEXT.md` is updated when a domain term is resolved.

## Layout

This repo is **single-context**:

```text
/
├── AGENTS.md
├── README.md
├── README.zh-CN.md
├── CONTRIBUTING.md
├── GOVERNANCE.md
├── SECURITY.md
├── CHANGELOG.md
├── CONTEXT.md          ← project glossary
├── src/                ← config, matcher, script, server, upstream modules
├── tests/              ← end-to-end checks through the built binary
├── docs/
│   ├── README.md
│   ├── development.md
│   ├── contracts/
│   └── agents/
├── plans/
│   ├── README.md
│   ├── product-definition.md
│   ├── demo-document-catalog.md
│   └── adr/
│       ├── README.md
│       └── 0001-...md
└── research/
    └── README.md
```

## Use the glossary's vocabulary

When your output names a domain concept (in an issue title, a refactor proposal, a hypothesis, a test name), use the term as defined in `CONTEXT.md`. Don't drift to synonyms the glossary explicitly avoids.

If the concept you need isn't in the glossary yet, that's a signal — either you're inventing language the project doesn't use (reconsider) or there's a real gap (note it for `/domain-modeling`).

## Flag ADR conflicts

If your output contradicts an existing ADR, surface it explicitly rather than silently overriding:

> _Contradicts ADR-0007 (project naming) — but worth reopening because…_

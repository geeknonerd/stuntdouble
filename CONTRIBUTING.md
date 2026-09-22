# Contributing to Stunt Double

This document describes how to contribute code and changes to the repository. Please read it before opening issues or submitting changes.

## Project status

The first six slices are implemented: `stuntdouble serve` and `stuntdouble validate` run against a TOML configuration, matched routes execute their JavaScript in the embedded Boa runtime with a host-injected `ctx` (including allowlisted `ctx.http.get` and streaming `ctx.http.pipe`), and every response carries a `request_id`. Unmatched routes answer 404 `not_found`; script failures answer 500 `script_error` or `script_no_response`; uncaught upstream transport failures answer 502 `upstream_unreachable`. The document manifest and PDF download scenarios from [plans/demo-document-catalog.md](plans/demo-document-catalog.md) run from [demo/](demo/). Local static-file reads, file streaming, and uploads land in later slices. Product definition, architecture decisions (ADRs), and research notes remain the design source of truth.

## Scope alignment

Stunt Double has a deliberately narrow v1 scope to avoid feature creep. Before proposing a new capability:

- Read [plans/product-definition.md](plans/product-definition.md) for the confirmed v1 range and non-goals.
- Check [plans/adr/](plans/adr/) for related architectural decisions. If your proposal conflicts with an ADR, note it explicitly in your discussion.
- Keep proposals consistent with the single execution model: `match → source → transform → response`. Avoid introducing a second engine or separate pipeline.

## Code of conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md). Report unacceptable behavior through the private reporting channel described in that document.

## Issue workflow

Use GitHub's issue templates for bugs and features. Labels we expect you to apply are defined in [docs/agents/issue-tracker.md](docs/agents/issue-tracker.md).

- For bug reports, include minimal reproduction steps, expected vs actual behavior, environment details, and redacted logs (never include customer hostnames, tokens, headers, request/response bodies).
- For enhancements, describe the problem, proposed solution, alternatives considered, and which part of the pipeline it affects (`match`, `source`, `transform`, `response`, or runtime).

Before opening an issue, ensure you can describe the question without exposing sensitive data. See our privacy rule below.

## Design documents (ADRs)

When a decision is hard to reverse (e.g., runtime choice, security boundaries, failure semantics), it is recorded as an Architecture Decision Record (ADR). ADRs are written in Chinese; propose the decision in an issue or discussion and the maintainer records it.

- Place the file under `plans/adr/` using kebab-case with the pattern `NNNN-slug.md`.
- Use this template header:

```md
- 状态：已接受 | 提议 | 推迟 | 拒绝
- 日期：YYYY-MM-DD
- 作者：@github_handle
- 关联：issue、讨论、其他 ADR 的链接
...
```

We maintain 13 ADRs so far. Add additional ones when needed.

## Development workflow

Use trunk-based development with short-lived branches and pull requests. `main` is always releasable. Squash merge is the only merge method; branches are deleted after merge. Every pull request must pass `fmt`, `clippy`, `test`, `docs`, `docs-links`, `deny`, `audit`, `msrv`, `codeql`, `pr-title`, and `dco`.

See [docs/development.md](docs/development.md) for the full workflow, versioning rules, and release process.

## Commit style

- Use **Conventional Commits** with PR titles that become the squash commit subject.
  Example: `feat(config): add route validation` or `fix(sandbox): reject path traversal`.
- Allowed types: `feat`, `fix`, `docs`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`, `security`.
- Scope is optional but recommended when it matches our areas: `config`, `match`, `source`, `transform`, `response`, `sandbox`, `runtime-js`, `runtime-python`, `files`, `obs`, `dist`.
- Signing-off with `git commit -s` (DCO) is mandatory on every commit.

## License and DCO

By contributing, you agree that your contribution is licensed under the dual license MIT OR Apache-2.0 unless you state otherwise in your PR description.

You also confirm you have authority over the submitted material (it is your original work, or you obtained permission from the copyright holder). Accept the developer certificate by signing commits with `git commit -s`.

Privacy rules:

- Do not include customer/hostnames, secrets, credentials, API keys, production logs, request/response bodies, or any other sensitive data in issues, PR descriptions, comments, or screenshots.
- When referencing internal data sources in an issue, use the public demo values from `plans/demo-document-catalog.md` (`/demo/documents/...`, `DOC-0001`, `metadata.example.com`).

Contributions will be reviewed against these rules before being accepted.

## Documentation language

English is the authoritative language for every document in the repository. Chinese translations live next to the English file with a `.zh-CN.md` suffix, and both pages carry a language switch line under the title.

- **English only:** `CONTRIBUTING.md`, `GOVERNANCE.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md`, `CHANGELOG.md`, the `.github/` templates, and commit and pull request titles.
- **English plus a Chinese translation:** the README pair, the GitHub Pages landing page, `docs/guide/`, `docs/contracts/`, and `demo/README.md`.
- **Chinese only:** `AGENTS.md`, `CONTEXT.md`, `plans/`, `research/`, `docs/development.md`, `docs/agents/`, and `docs/solutions/`. Design documents are written in Chinese because the maintainer works in Chinese.

You only need to write the English side. Do not update a `.zh-CN.md` translation as part of an external contribution: the maintainer keeps translations in sync and reviews them before each release. The full rules live in [docs/development.md](docs/development.md); the decision is recorded in [ADR 0013](plans/adr/0013-documentation-language-and-bilingual-structure.md).

## Security reporting

Report vulnerabilities via GitHub private vulnerability reporting at this repository. Do not paste exploit details into a public issue. See [SECURITY.md](SECURITY.md) for expectations.

Thank you for helping us build a reliable mock server for integration testing.

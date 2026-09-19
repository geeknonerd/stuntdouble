# Contributing to Stunt Double

This document describes how to contribute code and changes to the repository. Please read it before opening issues or submitting changes.

## Project status

The project is in design phase: there is no runnable server yet. The repository currently contains product definition, architecture decisions (ADRs), and research notes.

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

When a decision is hard to reverse (e.g., runtime choice, security boundaries, failure semantics), record it as an Architecture Decision Record (ADR):

- Place the file under `plans/adr/` using kebab-case with the pattern `NNNN-slug.md`.
- Use this template header:

```md
- Status: Accepted | Proposed | Deferred | Rejected
- Date: YYYY-MM-DD
- Authors: @github_handle(s)
- Related: links to issues, discussions, other ADRs
...
```

We maintain 8+ ADRs so far. Add additional ones when needed.

## Commit style

- Keep commit messages in English and concise (one line summary; optional context below).
- Conventional commits are welcome but not required. We recommend `chore:`, `docs:`, `feat:` prefix patterns.
- Sign off each commit to accept the DCO: `git commit -s`.

## License and DCO

By contributing, you agree that your contribution is licensed under the dual license MIT OR Apache-2.0 unless you state otherwise in your PR description.

You also confirm you have authority over the submitted material (it is your original work, or you obtained permission from the copyright holder). Accept the developer certificate by signing commits with `git commit -s`.

Privacy rules:

- Do not include customer/hostnames, secrets, credentials, API keys, production logs, request/response bodies, or any other sensitive data in issues, PR descriptions, comments, or screenshots.
- When referencing internal data sources in an issue, use the public demo values from `plans/demo-document-catalog.md` (`/demo/documents/...`, `DOC-0001`, `metadata.example.com`).

Contributions will be reviewed against these rules before being accepted.

## Documentation language

Root community documentation (README, LICENSE, CONTRIBUTING, SECURITY, CODE_OF_CONDUCT, issue/PR templates) should be written in English. Design docs (product definitions, ADRs, research) may be in Chinese now; English translations are welcome.

## Security reporting

Report vulnerabilities via GitHub private vulnerability reporting at this repository. Do not paste exploit details into a public issue. See [SECURITY.md](SECURITY.md) for expectations.

Thank you for helping us build a reliable mock server for integration testing.

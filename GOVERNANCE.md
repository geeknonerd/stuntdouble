# Governance

## Current model

Stunt Double is maintained by `@geeknonerd`. The maintainer owns merge, release, label, and security decisions.

The project uses a lightweight model while it is in early development. Decisions are made in public issues and recorded in ADRs when they are hard to reverse.

## Maintainer responsibilities

- Keep `main` releasable.
- Review and merge pull requests.
- Triage issues and apply labels.
- Cut releases and publish release notes.
- Respond to security reports.
- Keep the roadmap and public contracts current.

## Response expectations

These are expectations, not an SLA:

| Surface | Target |
| --- | --- |
| Security report | Acknowledge within 48 hours |
| Bug report | Triage within 7 days |
| Pull request | Review or status within 7 days |
| Feature request | Scope decision when capacity allows |
| Inactive issue | Mark `needs-info` after 90 days; close after 30 more days |

## Decision process

- Small changes: pull request and review.
- Hard-to-reverse changes: ADR under `plans/adr/`.
- Public contract changes: update the matching file under `docs/contracts/` and the CHANGELOG.
- Security-sensitive changes: private review before disclosure.

## Becoming a maintainer

A contributor may be invited after sustained, high-quality contributions and reliable review behaviour. The first additional maintainer triggers these changes:

- required approval count becomes one
- CODEOWNERS splits review ownership by area
- release permissions move to a release team
- governance document is updated

## Code of conduct

The project follows the [Contributor Covenant](CODE_OF_CONDUCT.md). Reports go through the private channel described there.

## Commercial work

Open-core and funding decisions are recorded in [ADR 0009](plans/adr/0009-open-core-and-funding.md). Commercial work must not fork the core execution model or hold back security fixes.

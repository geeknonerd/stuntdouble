# ADR 0009: Open-core and funding model

- Status: Accepted
- Date: 2026-09-19
- Related: [ADR 0006](0006-product-positioning.md), [ADR 0008](0008-dual-mit-apache-license.md), [open-source baseline](../../research/open-source-repo-baseline.md)

## Context

Stunt Double needs a sustainable funding path, but the core product is a local/CI developer tool. The project should not depend on donations alone, and it should not cripple the open-source core to force payment.

Comparable projects show several viable paths:

- WireMock Cloud and Mockoon Cloud sell hosted collaboration, deployment, and team workflows around an open-source engine.
- Stoplight sells a broader API design platform around Prism.
- Many Rust and CLI projects rely on sponsorship, support contracts, or corporate backing rather than paid feature gates.

## Decision

Use an **open-core** model with these revenue paths, in priority order:

1. **Support and SLA.** Paid support, prioritised fixes, upgrade assistance, and security review for teams running Stunt Double in CI or internal platforms.
2. **Hosted team service.** A paid control plane for shared route configuration, environment management, history, and team collaboration. The local binary remains fully usable without it.
3. **Enterprise governance.** SSO, audit logs, policy enforcement, central configuration, and compliance reporting for larger organisations.
4. **Training and integration services.** Workshops, migration from json-server middleware or WireMock, and custom upstream adapters.
5. **Sponsorship and donations.** Optional and supplemental. A "buy me a coffee" link may be added later, but it is not the primary plan.

## Constraints

- The open-source core must remain useful without a paid account.
- Security fixes and sandbox boundaries must never be held back for a paid tier.
- Paid features must sit above the single execution model, not fork it.
- No contributor license agreement (CLA) by default. Use DCO sign-off. Revisit only if the project needs to relicense later.
- Do not publish a funding link until there is a real account, a clear use of funds, and a maintainer who can handle payouts and tax obligations.

## Signals that would change this decision

- A large company offers sustained engineering sponsorship without feature restrictions.
- Hosted operations turn out to be the only viable business, making a stronger network or license boundary necessary.
- The project fails to gain adoption and support demand never appears; in that case keep the core free and treat the project as a portfolio or community effort.

## Consequences

- Commercial work must stay separated from the core repository and its release process.
- The project needs a public roadmap that distinguishes open-source core work from commercial work.
- Donations can cover infrastructure and small maintenance costs but should not be presented as a business model.

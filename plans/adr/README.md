# Architecture Decision Records

Architecture decisions use sequential numbering and kebab-case filenames.

| ADR | Title | Status |
| --- | --- | --- |
| [0001](0001-no-shared-state-in-v1.md) | No shared state in v1 | Accepted |
| [0002](0002-route-model-only-in-v1.md) | Route model only in v1 | Accepted |
| [0003](0003-script-first-multi-runtime.md) | Script-first transforms and multiple runtimes | Accepted |
| [0004](0004-host-functions-only-sandbox.md) | Host functions only in the sandbox | Accepted |
| [0005](0005-upstream-failure-semantics.md) | Upstream failure semantics | Accepted |
| [0006](0006-product-positioning.md) | Product positioning | Accepted |
| [0007](0007-naming-and-brand.md) | Naming and brand | Accepted |
| [0008](0008-dual-mit-apache-license.md) | Dual MIT OR Apache-2.0 license | Accepted |
| [0009](0009-open-core-and-funding.md) | Open-core and funding model | Accepted |
| [0010](0010-git-and-release-workflow.md) | Trunk-based development and release flow | Accepted |
| [0011](0011-contract-compatibility.md) | Separate compatibility contracts | Accepted |
| [0012](0012-release-artifacts-and-supply-chain.md) | Verifiable release artifacts and supply chain | Accepted |

## Adding an ADR

Use the next number and a kebab-case filename, for example `0013-admin-api-authentication.md`. Keep the record focused on one decision and link related issues or ADRs.

## When to write one

Write an ADR when the decision is hard to reverse, surprising without context, and the result of a real trade-off. Skip it when the decision is easy to reverse or has no meaningful alternative.

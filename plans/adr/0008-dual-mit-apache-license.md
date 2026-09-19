# ADR 0008: Dual-license the core under MIT OR Apache-2.0

- Status: Accepted
- Date: 2026-09-19
- Related: [ADR 0006](0006-product-positioning.md), [ADR 0007](0007-naming-and-brand.md), [open-source baseline](../../research/open-source-repo-baseline.md)

## Context

Stunt Double is a developer tool intended for local and CI use. It must be easy for companies and individual developers to adopt, embed, and redistribute. The project may later offer commercial services, but the core should not be held back by a restrictive license.

The main candidates were:

- MIT
- Apache-2.0
- Dual MIT OR Apache-2.0
- AGPL-3.0

## Decision

License the Stunt Double core under **MIT OR Apache-2.0**, at the user's option.

The repository carries both license texts as `LICENSE-MIT` and `LICENSE-APACHE`. Contributions are accepted under the same dual license unless explicitly stated otherwise.

## Why

- **Rust ecosystem norm.** Rust itself, clap, bat, Biome, and many crates use Apache-2.0 or MIT OR Apache-2.0. Rust users already understand the combination.
- **Commercial adoption.** MIT and Apache-2.0 are permissive and allow internal enterprise use, redistribution, and commercial products without legal review friction.
- **Patent protection.** Apache-2.0 includes an explicit patent grant, which matters for infrastructure software.
- **Compatibility.** MIT is GPL-compatible. Apache-2.0 is compatible with many corporate policies and includes a clear contribution model.
- **No lock-in.** The project can still monetize support, hosting, and enterprise features without restricting the core.

## Rejected alternatives

- **MIT only.** Simpler, but gives up Apache-2.0's explicit patent grant and corporate-friendly language.
- **Apache-2.0 only.** Also strong, but the dual license gives users the shortest possible permissive option and matches Rust convention more closely.
- **AGPL-3.0.** Strong protection against SaaS free-riding, but creates adoption friction for the local/CI developer workflow and does not match the current product strategy. Revisit only if a future hosted product needs a different boundary.
- **Source-available or Business Source License.** Rejected. It would block open-source adoption and community contribution for a tool that benefits from being available in CI and containers.

## Consequences

- The core can be packaged by downstream distributions and used in commercial environments.
- The project cannot unilaterally relicense existing contributions without contributor agreement.
- Commercial value must come from services and adjacent products, not from restricting the core.
- New dependencies must be compatible with MIT OR Apache-2.0 distribution.

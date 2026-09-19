# Open-source repository baseline

- Date: 2026-09-19
- Purpose: choose a license and funding model, and align the repository with common open-source project conventions.

## Comparable projects and licenses

| Project | License | Notes |
| --- | --- | --- |
| [Rust](https://github.com/rust-lang/rust) | Apache-2.0 | Systems language with a corporate-friendly license and explicit patent grant. |
| [clap](https://github.com/clap-rs/clap) | Apache-2.0 | Rust CLI library with broad commercial adoption. |
| [bat](https://github.com/sharkdp/bat) | Apache-2.0 | Rust CLI tool with a permissive license. |
| [Biome](https://github.com/biomejs/biome) | Apache-2.0 | Rust developer tool with a permissive license. |
| [Tokio](https://github.com/tokio-rs/tokio) | MIT | Rust async runtime with a short permissive license. |
| [Deno](https://github.com/denoland/deno) | MIT | JavaScript runtime with a short permissive license. |
| [WireMock](https://github.com/wiremock/wiremock) | Apache-2.0 | Mock server with a hosted commercial product, WireMock Cloud. |
| [Mockoon](https://github.com/mockoon/mockoon) | MIT | Mock server with a hosted Mockoon Cloud product and sponsor program. |
| [Prism](https://github.com/stoplightio/prism) | Apache-2.0 | API mocking and validation tool inside the Stoplight commercial platform. |
| [json-server](https://github.com/typicode/json-server) | MIT | Minimal mock data server with a large adoption base. |

## License conclusion

For a Rust developer tool that wants broad adoption and a future hosted or enterprise business, the strongest default is **MIT OR Apache-2.0**.

- MIT keeps the shortest possible permissive path for users and downstream distributions.
- Apache-2.0 adds an explicit patent grant and corporate-friendly language.
- The combination matches Rust ecosystem expectations.
- AGPL-3.0 would reduce adoption friction for a local/CI tool and does not fit the current product strategy.

Recorded in [ADR 0008](../plans/adr/0008-dual-mit-apache-license.md).

## Funding conclusion

Donations alone are usually not a sustainable funding model for infrastructure tooling. The practical options seen in comparable projects are:

1. Hosted service around the open-source engine.
2. Support contracts and SLAs.
3. Enterprise governance and collaboration features.
4. Training and integration services.
5. Sponsorship and donations as supplemental income.

Recorded in [ADR 0009](../plans/adr/0009-open-core-and-funding.md).

## Repository conventions worth adopting

GitHub's community profile recommends these files for public repositories:

- `README.md`
- `LICENSE` files
- `CONTRIBUTING.md`
- `CODE_OF_CONDUCT.md` (when a reporting contact exists)
- `SECURITY.md`
- Issue templates
- Pull request template

This repository now includes README (English and Chinese), dual license files, CONTRIBUTING, CODE_OF_CONDUCT, SECURITY, issue templates, and a pull request template. The code of conduct uses the repository private reporting form until a dedicated conduct email exists.

## Sources

- GitHub Docs, "About community profiles for public repositories": https://docs.github.com/en/communities/setting-up-your-project-for-healthy-contributions/about-community-profiles-for-public-repositories
- GitHub Docs, "Licensing a repository": https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/licensing-a-repository
- Choose a License, MIT: https://choosealicense.com/licenses/mit/
- Choose a License, Apache-2.0: https://choosealicense.com/licenses/apache-2.0/
- WireMock Cloud: https://www.wiremock.io/
- Mockoon Cloud: https://mockoon.com/cloud/
- Open Source Guides, "Starting an Open Source Project": https://opensource.guide/starting-a-project/

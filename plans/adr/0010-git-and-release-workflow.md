# ADR 0010: Trunk-based development and release flow

- Status: Accepted
- Date: 2026-09-19
- Related: [ADR 0008](0008-dual-mit-apache-license.md), [development guide](../../docs/development.md)

## Context

The repository needs a predictable workflow before implementation starts. The current repository has one `main` branch, no tags, no releases, and no branch protection. Releases will need to be automated and reproducible across three platforms.

The candidates were trunk-based development, GitHub Flow with merge commits, and GitFlow.

## Decision

Use trunk-based development with squash-only merges.

- `main` is always releasable.
- All work goes through short-lived branches and pull requests.
- Squash merge is the only merge method; branches are deleted after merge.
- `main` is protected against direct pushes, force pushes, and deletion.
- CI is the hard gate. While there is one maintainer, the required approval count is zero; it becomes one when a second maintainer joins.
- Releases are tagged from `main`. A `develop` branch is not used.
- After 1.0, older lines use `release/x.y` branches created from tags.
- Release PRs come from `release-plz`; merging the release PR authorises the tag and publication.
- A failed release never reuses or overwrites a tag. It ships a new patch or pre-release.

## Rejected alternatives

- **GitHub Flow with merge commits.** Simpler, but noisy history and weaker CHANGELOG automation.
- **GitFlow.** Suitable for parallel maintained versions, too heavy for the current project.
- **No defined workflow.** Incompatible with automated releases and branch protection.

## Consequences

- Pull request titles must follow Conventional Commits because the squash commit inherits them.
- DCO sign-off is mandatory.
- Branch protection and required status checks become part of the release contract.
- Release automation must run from tags, never from `main` directly.

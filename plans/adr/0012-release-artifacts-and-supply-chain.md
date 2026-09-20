# ADR 0012: Verifiable release artifacts and supply-chain baseline

- Status: Accepted
- Date: 2026-09-19
- Related: [ADR 0008](0008-dual-mit-apache-license.md), [ADR 0010](0010-git-and-release-workflow.md), [development guide](../../docs/development.md)

## Context

Stunt Double is intended for CI and local development, where users need trustworthy binaries and containers. A release that cannot be traced back to a tag or verified after download creates avoidable supply-chain risk.

## Decision

Every release provides a verifiable artifact set:

- source archives from GitHub
- Linux x86_64, macOS arm64, and Windows x86_64 binaries
- `ctx` API `.d.ts` type definitions for every supported `apiVersion`
- `SHA256SUMS`
- GitHub artifact attestation
- SBOM in CycloneDX or SPDX format
- container image at `ghcr.io/geeknonerd/stuntdouble:<tag>` with a published digest
- GitHub Release notes generated from the CHANGELOG

Build artifacts only from tags. Never publish binaries built from `main`.

Signing starts with GitHub artifact attestation. Add Sigstore/cosign only when offline verification becomes a requirement.

Container images use a multi-stage build and a minimal runtime image. Tags and digests are both published; `latest` is never the only reference.

A failed release does not overwrite artifacts. Fix the issue and publish a new version. Use `cargo yank` only for a broken crates.io release.

## Tooling

- `release-plz` for release PRs, versions, CHANGELOG, tags, and optional crates.io publication.
- `cargo-dist` for multi-platform binaries, installers, checksums, and GitHub Release assets.
- GitHub Actions for CI and release workflows.
- `cargo-deny` and `cargo-audit` for dependency and advisory checks.
- Dependabot for Cargo and GitHub Actions updates.

## Activation

The release workflows and `cargo-dist` configuration are added in the first implementation slice, before the first public release. Until then, this ADR defines the required shape rather than an active pipeline.

## Consequences

- Release automation must have access to repository and package-registry secrets.
- Artifacts are reproducible from tags and a locked dependency graph.
- Release notes are part of the release artifact, not an afterthought.

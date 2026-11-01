## What this changes

<!-- One or two sentences. Link the issue it closes, if there is one. -->

## Why

<!-- The problem this solves. -->

## Checklist

- [ ] `cargo fmt --all --check` passes
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes
- [ ] `cargo test --all` passes
- [ ] Tests cover the change
- [ ] `CHANGELOG.md` has a line under `## [Unreleased]`, if a user would notice
- [ ] Commits follow Conventional Commits
- [ ] This change is platform-sensitive and needs the macOS job run against it

<!--
The macOS job runs on `main` and on manual dispatch, not on pull requests.
Tick the last box if a maintainer should run it before merging.
-->

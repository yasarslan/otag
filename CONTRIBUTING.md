# Contributing to otag

Thanks for your interest. otag is pre-alpha, so the fastest way to help is to
open an issue describing what you want to do before writing a large patch.

## Requirements

- A stable Rust toolchain, 1.88 or newer (the minimum supported Rust version is
  declared as `rust-version` in `Cargo.toml`).
- macOS or Linux. Windows is not supported yet.

## Build and test

```sh
cargo build
cargo test
```

Before opening a pull request, run the same checks CI runs:

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo deny check          # cargo install cargo-deny
cargo doc --no-deps --all-features
```

Warnings are errors: `cargo clippy` must be clean, and `cargo doc` must build
without warnings.

## Code expectations

- **The daemon never crashes because of one bad input.** No `unwrap()` or
  `expect()` on daemon code paths; log the error and carry on. Tests may use
  `expect` freely.
- **Errors are written for users.** Say what happened and what to do about it.
- **Parsers are driven by fixtures.** Anything that reads a file format or a
  payload produced by another program gets tests against a sample in
  `tests/fixtures/`, and treats every field as optional.
- **Tests do not touch your real home directory or configuration.** Pass paths
  explicitly, give child processes their own `HOME`, and do not mutate the test
  process's environment — it is shared by every test running in parallel.
- Public modules, types and functions carry doc comments.

## Dependencies

New dependencies must pass `cargo deny check`, which enforces the allowed
licenses in `deny.toml`. Prefer small, well-maintained crates. Code copied from
GPL- or AGPL-licensed projects cannot be accepted.

## Commits

Commits follow [Conventional Commits](https://www.conventionalcommits.org/):

```
feat: add the approval inbox drawer
fix: keep the daemon alive when a pane dies
docs: document the socket protocol
```

Use `feat`, `fix`, `docs`, `refactor`, `test`, `chore` or `ci`. One logical
change per commit, with a message that explains the change rather than the
files it touched.

## Pull requests

1. Branch from `main`.
2. Keep the change focused, and add tests for it.
3. Add a line under `## [Unreleased]` in `CHANGELOG.md` for anything a user
   would notice.
4. Make sure the checks above pass.

CI runs the full suite on Linux for every pull request, and on macOS for
changes that land on `main`. If your change is platform-sensitive, say so in
the pull request so a maintainer can run the macOS job against it.

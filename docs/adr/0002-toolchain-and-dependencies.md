# 0002: Toolchain, MSRV, and dependency policy

- **Status:** Accepted
- **Date:** 2026-10-18

## Context

Builds, lints, and formatting must be reproducible for every contributor and
in CI. Assemblr is an end-user CLI, so a minimum supported Rust version (MSRV)
matters mainly for people who build it from source.

## Decision

### Toolchain

- **Edition:** Rust 2024.
- **Development toolchain:** pinned to **Rust 1.99.0** in `rust-toolchain.toml`,
  with `rustfmt` and `clippy`. This was the current stable release when the
  repository was founded (2026-10-18). Pinning keeps lint and format results
  identical locally and in CI.
- **MSRV:** **Rust 1.85**, declared as `rust-version` in `Cargo.toml`. This is
  the minimum Rust release that supports edition 2024, and it matches the MSRV
  of `clap` 4.6. CI verifies it with `cargo +1.85.0 test --locked`.
- Edition 2024 uses Cargo resolver 3, which prefers dependency versions
  compatible with `rust-version` when generating `Cargo.lock`.

Bumping the pinned toolchain is a routine change. Raising the MSRV needs a
stated reason, such as a required language feature or dependency, and the CI
MSRV job must be updated at the same time.

### Lints

`Cargo.toml` `[lints]` enforce:

- `unsafe_code = "forbid"`. Safe Rust only. Lifting this requires an ADR.
- `unreachable_pub`, to keep public visibility minimal.
- Clippy `unwrap_used`, `expect_used`, `todo`, `unimplemented`, `dbg_macro`.
  These prevent panics on user-controlled data and placeholder
  implementations. `clippy.toml` allows `unwrap`/`expect` in tests.

CI treats all warnings as errors (`-D warnings`).

### Dependencies

A dependency is added only when it provides **immediate** value to
implemented functionality. Current dependencies:

| Crate | Purpose |
|-------|---------|
| `clap` (derive) | CLI argument parsing, `--help`, `--version` |

No async runtime, dependency-injection framework, plugin system, merge library,
or Git library is added in anticipation of future work. Integration tests use
`std::process::Command` with `CARGO_BIN_EXE_assemblr` rather than a
test-harness crate.

`Cargo.lock` is committed, and CI builds with `--locked`.

### CI

GitHub Actions on Linux and Windows run format, clippy, build, and test checks,
plus an MSRV job on Linux. macOS is not in CI yet. Add it when there is a
platform-specific concern or a user need.

The workflow has run successfully on GitHub-hosted Linux and Windows runners,
including the MSRV job. The toolchain install step (`rustup toolchain install`
with no arguments, which installs the toolchain pinned in
`rust-toolchain.toml`) requires rustup 1.28 or later on the runner.

## Consequences

- Contributors get the pinned toolchain automatically through rustup.
- Raising either toolchain version is an explicit, reviewable change.
- A new dependency is a reviewable decision. Significant ones get an ADR or an
  entry in this one.

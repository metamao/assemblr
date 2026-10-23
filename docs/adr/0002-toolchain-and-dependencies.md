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
| `semver` | Exact versions and version requirements, with Cargo's requirement semantics, the provisional range dialect (OC-15). Wrapped in domain newtypes, so it stays out of the public API. |
| `serde` (no default features except `std`), `serde_json` | Parsing manifest JSON: a recursion limit, syntax errors with line and column, and a hand-written visitor that keeps duplicate keys so they can be reported. |
| `sha2` (no default features) | SHA-256 for the identity of a consumer project's reconciliation base and of each stored file (ADR 0016): the owner chose that algorithm on #41. Pure Rust, about ten crates in the tree (`digest`, `generic-array`, `typenum`, `cpufeatures`, `block-buffer`, `crypto-common`, `cfg-if`, `version_check`, `libc` on some targets), MSRV 1.72. Writing a hash function by hand was not considered. |

Manifests are JSONC. `jsonc-parser` was evaluated first. Its releases from
0.28 on need Rust newer than 1.85, and 0.26.2, which builds on 1.85, has no
nesting limit and lacks most of the switches that turn off non-JSON syntax.
Raising the MSRV for one parser was not justified. Instead, a small adapter
blanks out comments and trailing commas while preserving every byte offset,
and passes the result to `serde_json`.

No async runtime, dependency-injection framework, plugin system, merge library,
or Git library is added in anticipation of future work. Integration tests use
`std::process::Command` with `CARGO_BIN_EXE_assemblr` rather than a
test-harness crate.

Development dependencies follow the same rule, weighed against what they
add to the test build:

| Crate | Purpose |
|-------|---------|
| `regex-lite` (dev) | Compiles the patterns of the embedded manifest JSON Schema in tests, so that they are checked against the domain's parsers of identities, versions, and parameter names (ADR 0014). Pure Rust, no dependencies, MSRV 1.65. A full JSON Schema validator (`jsonschema`, about 75 crates) was rejected for a schema that is editor assistance only. |

`Cargo.lock` is committed, and CI builds with `--locked`.

### CI

GitHub Actions on Linux and Windows run format, clippy, build, and test checks,
plus an MSRV job on Linux. To stay within the Actions minutes of a private
repository (Windows minutes count double), pull requests run the Linux checks
only; Windows and the MSRV run on pushes to `main` and when the workflow is
started by hand, and changes to documentation alone (`docs/**`, `*.md`) skip
CI. macOS is not in CI yet. Add it when there is a
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

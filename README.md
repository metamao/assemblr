# Assemblr

Assemblr is a Rust CLI for composing and maintaining opinionated, reusable
software **units**: versioned, author-owned implementations of software
capabilities and integrations, such as `@metamao/cloudflare-worker`.

Units are composed with their dependencies. They modify dependency files
through ordinary full-file copies (*amendments*), are validated by Assemblr,
and are published as immutable, versioned Git tags. Assemblr is
technology-agnostic and built for both humans and coding agents.

## Status

**Early implementation.** This repository holds the canonical domain
contracts and the production implementation, which is built in increments
tracked in GitHub [issue #9](https://github.com/metamao/assemblr/issues/9).
The library reads and validates unit manifests. The CLI currently responds
only to `--help` and `--version`.

## Documentation

| Where | What |
|-------|------|
| [`docs/domain/`](docs/domain/README.md) | What Assemblr means and promises: the canonical product contracts |
| [`docs/domain/open-contracts.md`](docs/domain/open-contracts.md) | Product questions that are not yet decided |
| [`AGENTS.md`](AGENTS.md) | How Assemblr is engineered: rules for contributors and coding agents |
| [`docs/adr/`](docs/adr/README.md) | Architectural decision records |

## Development

The toolchain is pinned in `rust-toolchain.toml`, and rustup installs it
automatically.

```sh
cargo run -- --help
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo build --all-targets
```

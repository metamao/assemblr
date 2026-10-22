# 0028: Isolated validation: shared test fixtures, a sandbox for manual runs, and a reference scenario

- **Status:** Accepted
- **Date:** 2026-10-22

## Context

AGENTS.md §8 requires tests that never read or change the developer's
registry, Git configuration, or repositories. Integration tests met it file
by file: ten files each copied the same Git helper and fixture, and they had
drifted. Five ran the binary with the system Git configuration and inherited
`GIT_*` variables, and every fixture lived under `target/`, inside this
repository's working tree, where Git reads this repository's ignore rules
and finds this repository from any fixture directory that has none of its
own.

Validation by hand had no support at all. Agents rebuilt scratch registries
in each session, and because shell state does not persist between an agent's
commands, one run reached the user's real registry when a single command
lacked `ASSEMBLR_DATA_DIR` (agent retrospective assessment, 2026-10-22). An
instruction to remember the variable fails the same way, so isolation has to
come from how the command is run.

## Decision

1. **`tests/common/mod.rs` holds the isolation.** One `Fixture` (a test's
   directory, registry, and home), the `git` and `assemblr` commands, and
   the output helpers. Each test file keeps its own scenario helpers.
   - Fixtures live in `assemblr-tests-<hash>/<test crate>/<test>` under the
     system's temporary directory, outside any repository. The hash is of
     the checkout's path, so two worktrees never share a directory.
   - `git` and `assemblr` run without the caller's `GIT_*` variables and
     without the system Git configuration. `git` uses no global
     configuration, fixed identities and dates, and a ceiling at the tests'
     root. `assemblr` uses `home/.gitconfig` as its global configuration, so
     a test that needs one writes it there.
2. **`cargo sandbox` runs commands against a development build in an isolated
   environment.** It is a Cargo alias for `examples/sandbox.rs`, so it needs
   nothing but Cargo on Linux, macOS, and Windows. A named sandbox is a
   directory under the system's temporary directory that keeps its registry,
   working directory, temporary directory, and a Git configuration with an
   identity and no signing. Each run builds the binary, puts it first on
   `PATH`, removes inherited `GIT_*` variables, and sets the environment
   again, so a scenario can run one command per call. It refuses to run when
   the sandbox would be inside a Git repository. `--reset` starts over and
   `--name` selects another sandbox.
3. **`cargo run` defaults to a registry under `target/dev-data`**, through
   `[env]` in `.cargo/config.toml`, unless `ASSEMBLR_DATA_DIR` is already
   set. This is a backstop for a development run outside the sandbox. It
   does not isolate Git or the working directory.

4. **`tests/scenario.rs` replays the reference scenario.** Four small units
   modeled on EXP-12 (#50) are authored, amended, checked, published, and
   registered through the binary, then composed, edited, updated across two
   provider releases (a clean merge and a resolved conflict), and reduced by
   `remove`. It is an ordinary integration test: offline, a few seconds, on
   both CI platforms. The composed project's own toolchains (npm, Angular,
   Wrangler, Docker) are not run; EXP-12 does that with its full units, and
   its cost and network needs keep it out of this suite.

## Consequences

- A change to the isolation rules, such as a new Git variable, is made once.
- `cargo run` from this checkout never uses the user's registry. Someone who
  wants it sets `ASSEMBLR_DATA_DIR` or runs an installed binary. The release
  build and installed binaries are unaffected: the setting applies only to
  processes Cargo starts.
- Fixtures under the system's temporary directory are not cleaned by
  `cargo clean`. Each test removes its own directory before it runs, as
  before.
- The sandbox is a development tool, not part of the product: it is not
  shipped, and the binary knows nothing about it.

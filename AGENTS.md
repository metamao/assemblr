# AGENTS.md

Engineering instructions for anyone, human or agent, working on Assemblr.
This file defines **how Assemblr is engineered**.
[`docs/domain/`](docs/domain/) defines **what Assemblr means and promises**.

## 1. Product intent and documentation authority

Assemblr is a technology-agnostic Rust CLI that composes software projects
from versioned, reusable, author-owned **units**. It is a composition tool, not
an application framework. Humans and coding agents are both first-class users.

Authority, highest first:

1. Approved contracts in [`docs/domain/`](docs/domain/README.md).
2. This file, for engineering and architecture.
3. Implementation and tests, as evidence of actual behavior.
4. Historical research and experiments, as supporting evidence only.

Rules:

- **Never invent domain behavior.** If `docs/domain/` does not specify a
  behavior, it is unspecified. Check
  [`open-contracts.md`](docs/domain/open-contracts.md). If the question is
  not listed, add it there and ask the owner. Do not pick an answer and
  implement it.
- Statements in **Approved** sections are normative. Items marked
  **Conceptual** show intent, not syntax. Items marked **Open** must not be
  implemented by assumption. The register separates unresolved contracts,
  which need an owner decision, from approved semantics with open details.
  For the latter, implement the approved part. A detail that affects
  user-visible behavior, syntax, or file formats still needs an owner
  decision.
- Earlier prototypes and research experiments are historical evidence, not
  authority. Do not adopt their code, specifications, syntax (such as
  `extends`, `merges`, `fragments`, or `derived`/`scaffold` file classes), or
  tests unless `docs/domain/` adopts them explicitly.
- The consumer-project lifecycle is **reserved** (OC-1). Do not design or
  implement it without an owner decision.
- When approved semantics change, update `docs/domain/` in the same change as
  the code.

## 2. Architecture and dependency boundaries

See [ADR 0001](docs/adr/0001-architecture.md). In short: **functional core,
imperative shell**, informed by ports and adapters, in one Cargo package with
a library (`src/lib.rs`) and the `assemblr` binary (`src/main.rs`).

| Layer | Owns | Must not depend on |
|-------|------|--------------------|
| Domain | Identities, versions, invariants, resolution and merge semantics, conflict models | Application, adapters, CLI, `std::fs`, `std::process`, `std::env`, network, clocks |
| Application | Use cases, planning, orchestration | Concrete adapters, CLI |
| Adapters | Git, filesystem, registries, serialization, rendering, caching | CLI |
| CLI (binary) | Parsing, dispatch, diagnostics, output formatting | Adapter internals beyond what the library exposes |

- Dependencies point inward. Domain code takes data and returns data.
- The application defines the narrowest interface it needs from an external
  capability. Adapters implement it. Use concrete types until there is a
  second implementation or a test that needs a seam.
- CLI code lives in the binary target (`src/main.rs`, then `src/cli/` as it
  grows). The library never depends on `clap` or terminal concerns.
- Keep modules small and cohesive. Create a module only when real
  functionality needs it. No empty layers, placeholder traits, mock
  subsystems, or `todo!()` stubs for future features.
- No abstraction without a demonstrated requirement: no generic plugin
  systems, DI frameworks, or speculative extension points.
- One package. Moving to a workspace requires an ADR.

## 3. Determinism and pure domain computation

Given the same inputs and history, every domain computation produces the same
logical result. Never let semantics depend on:

- filesystem enumeration order: sort directory listings explicitly;
- hash-map iteration order: use `BTreeMap`/`BTreeSet` or sort before
  observable use;
- locale, time zone, the current time, or environment variables;
- incidental installation, configuration, or declaration order, unless a
  domain contract explicitly assigns meaning to it;
- hidden mutable global state.

Resolution, merges, conflict detection, diagnostics ordering, and
machine-readable output must all be deterministic. Write tests that would fail
if they were not, for example by feeding inputs in shuffled order.

## 4. Planning and persistent-state safety

- **Separate computing intended changes from applying them.** Compute a
  plan as data, validate it completely, detect all conflicts, and only then
  touch persistent state.
- Never leave a silent partial update. If a plan cannot be applied
  completely, report exactly what was and was not applied.
- Do not claim atomic or transactional file updates unless the mechanism is
  actually implemented and tested, for example write-to-temp plus rename,
  with its platform caveats documented.
- Authoring files (manifest, `src/`, `amendments/`) must never be damaged by a
  failed operation that targets `build/` or another derived location.
- Published content is immutable. Never rewrite tags or history that
  Assemblr has published.

## 5. Filesystem and external-input security

Treat manifests, unit files, repositories, registry contents, paths, Git
output, and subprocess output as **untrusted**.

- Validate and normalize every path taken from untrusted input. Reject
  absolute paths, `..` traversal, and platform-specific escapes (drive
  prefixes, UNC paths, reserved names on Windows).
- Writes into an owned directory such as `build/` must stay inside it.
  Protect against **symlink escapes**: do not follow symlinks out of the
  target root, and check containment on resolved paths, not just lexical ones.
- **Never execute code supplied by units**, implicitly or otherwise. No hooks,
  install scripts, or template-engine features that reach the shell or the
  filesystem.
- When invoking external programs such as `git`, pass arguments as a list
  (never through a shell), and set the working directory and environment
  explicitly.
- Bound resource use when reading untrusted data where practical.

## 6. Errors and diagnostics

- **Domain errors are typed.** Use enums that describe what went wrong in
  domain terms, for example an ambiguous source, an options conflict, or a
  dependency cycle, carrying the data needed to explain them.
- **Infrastructure failures carry context**: which file, repository, or
  operation failed, and why.
- **CLI diagnostics are actionable.** Say what failed, where, and what the
  user can do about it. Report every relevant conflict, not just the first,
  when that is practical.
- **Never panic on user-controlled or external data.** `unwrap`/`expect` are
  linted outside tests. A panic is acceptable only for a genuine internal
  invariant violation, with a message explaining the invariant.
- Errors go to stderr and data to stdout. Exit non-zero on failure.
  Machine-readable output (`--json`) must be stable and derived from the same
  domain data as human output.
- Error-handling crates (for example `thiserror`) may be added when the first
  real error types need them (see §7 on dependencies). Keep opaque error types
  out of the library API.

## 7. Idiomatic Rust standards

See [ADR 0002](docs/adr/0002-toolchain-and-dependencies.md).

- Edition 2024. The toolchain is pinned in `rust-toolchain.toml` (1.99.0). The
  MSRV is `rust-version` in `Cargo.toml` (1.85).
- **Safe Rust only** (`unsafe_code = "forbid"`).
- Idiomatic ownership: borrow by default, take ownership when you store,
  avoid gratuitous `clone()`. Prefer `&str`/`&Path` parameters.
- **Minimal visibility.** Default to private, then `pub(crate)`. `pub` only
  for the library's deliberate API (`unreachable_pub` is linted).
- Make invalid states unrepresentable. Validate at the boundary into
  newtypes (for example, a unit identity), then pass the validated types
  around.
- No async runtime unless a real requirement appears. No speculative
  generics.
- **Dependencies:** add a crate only for immediate value to implemented
  functionality. Prefer well-maintained crates with a compatible MSRV. Commit
  `Cargo.lock`. Record significant additions in an ADR.
- Doc comments on public items explain behavior and invariants. Comment the
  *why*, not the *what*.
- Formatting is `rustfmt` with defaults. Clippy warnings are errors.

## 8. Testing and verification

Before declaring any change complete, run:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo build --all-targets
```

CI ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)) runs these on Linux
and Windows with `--locked`, and checks the MSRV.

- Domain logic gets fast unit tests next to the code. CLI behavior gets
  integration tests in `tests/` that run the real binary.
- **Every defect fix includes a regression test** that fails without the fix.
- Tests are reproducible and offline. Build Git repositories and unit fixtures
  in temporary directories inside the test. Never depend on the network, the
  developer's global Git config, or machine-specific paths.
- Test conflict and error paths as thoroughly as success paths, and assert
  that diagnostics are useful.
- Assert on stable substrings or structured data rather than whole
  terminal-formatted output, so tests pass on every platform.

## 9. Scope discipline and decision records

- Implement only what the current task and approved contracts require.
- Do not ship non-functional commands. A command appears in `--help` only
  when its behavior exists.
- When an engineering decision affects architecture, dependencies, or
  cross-cutting conventions, record it in [`docs/adr/`](docs/adr/README.md).
- When work reveals an unspecified product question, add it to
  [`open-contracts.md`](docs/domain/open-contracts.md) and surface it. Do not
  resolve it silently.
- If a foundational technical constraint conflicts with the approved
  architecture, report the evidence before changing the architecture.

## 10. Using `rust-skills`

Agent tooling is optional. No skill, plugin, or MCP server is required to work
on this repository. Use the relevant skills and MCP tools when your environment
provides them. When it does not, fall back to ordinary repository inspection,
the standard Rust documentation, and the rules in this file.

When the `actionbook/rust-skills` plugin is available, use it for specialized
Rust guidance, for example:

- `rust-skills:m06-error-handling` and `rust-skills:m13-domain-error` for error
  design;
- `rust-skills:m05-type-driven` and `rust-skills:m09-domain` for validated
  domain types and invariants;
- `rust-skills:domain-cli` for CLI conventions;
- `rust-skills:m01-ownership`, `rust-skills:m04-zero-cost`, and
  `rust-skills:m15-anti-pattern` for ownership, generics, and code review;
- `rust-skills:coding-guidelines` for style;
- `rust-skills:unsafe-checker` if `unsafe` is ever proposed.

Skill guidance is advisory. It **never overrides** `docs/domain/`, this file,
or accepted ADRs. Where a skill suggests a crate or pattern (for example
`anyhow`, layered config, or progress bars), apply this file's dependency and
scope rules first.

## 11. Using `codebase-memory-mcp`

[codebase-memory-mcp](https://github.com/DeusData/codebase-memory-mcp) provides
a code knowledge graph. Prefer it for repository intelligence once the
codebase is substantive. It is an aid, **not** a source of product authority.

- **Use a graph rooted at this repository.** A graph indexed from a parent
  directory also contains unrelated code, whose symbols must not be mistaken
  for Assemblr's. Run `list_projects` and use the project whose root is this
  repository. If none exists, run `index_repository` on the repository root.
- **Check freshness first** with `index_status` (and `check_index_coverage`
  for files you rely on). Re-index with `index_repository` when the index is
  stale or missing.
- **Explore architecture** with `get_architecture`, `search_graph`, and
  `get_file_outline`.
- **Inspect call relationships** with `trace_path`.
- **Assess the impact of a change** with `detect_changes`.
- **Refresh the graph** after significant refactors when you will keep relying
  on it.
- **Confirm graph findings against the actual source and tests** before acting
  on them.
- **Skip indexing** for trivial changes or while the repository is nearly
  empty. Ordinary file reading and searching are enough.
- **Never use MCP graph storage or its ADR feature (`manage_adr`) as a
  substitute** for tracked documentation. Decisions go in `docs/adr/`, and
  contracts go in `docs/domain/`.
- If the server is unavailable, continue with ordinary repository inspection.

Tool names can differ between server versions. If a tool listed above is
missing, use the closest available equivalent or plain inspection.

## 12. Git workflow and integration

`main` is the integration branch. It is always green and releasable. These
rules apply whether or not the hosting platform enforces them.

- **Never commit or push directly to `main`.** Never force-push, delete
  remote branches, or rewrite history that has been pushed.
- Do all work on a short-lived branch named by intent: `feat/…`, `fix/…`,
  `docs/…`, `refactor/…`, `ci/…`, or `chore/…`. Keep each branch to one
  cohesive change.
- Run the checks in §8 locally before pushing, then push the branch and open
  a pull request. The description states what changed and why, which checks
  ran, and any change to `docs/domain/`, ADRs, or open contracts.
- **The owner reviews and merges.** Agents never merge pull requests,
  including their own. Changes to `docs/domain/` need the owner's explicit
  approval.
- Merge only when CI is green. Pull requests are squash-merged, so each lands
  on `main` as one commit, and the branch is deleted after merging.
- Commit messages have an imperative subject of about 72 characters or fewer,
  and a body that explains why when that is not obvious.
- **Releases** are `vX.Y.Z` tags of Assemblr itself on `main`, created by
  the owner. Agents do not create or push release tags. They are unrelated to
  the `v<version>` tags that units use for publication.

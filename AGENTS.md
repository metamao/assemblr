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
- Items marked **Recommended (provisional)** are defaults the owner has
  accepted for Alpha. Implementations may follow them, and doing so is not
  implementing by assumption. The related open contract stays open until the
  owner settles it. A recommendation the owner has not accepted is not a
  requirement.
- Earlier prototypes and research experiments are historical evidence, not
  authority. Do not adopt their code, specifications, syntax (such as
  `extends`, `merges`, `fragments`, or `derived`/`scaffold` file classes), or
  tests unless `docs/domain/` adopts them explicitly.
- The consumer-project lifecycle is specified in
  [`consumer-projects.md`](docs/domain/consumer-projects.md). Its open details
  are tracked in the register and must not be implemented by assumption.
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
  domain terms, for example a source conflict, an options conflict, or a
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
  developer's global Git config, or machine-specific paths. Never read or
  modify the developer's real Assemblr registry or configuration directories.
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
- Automated review (`claude-review.yml`) is advisory. Fix valid findings,
  reply with evidence to findings you believe are wrong, and escalate open
  domain decisions to the owner. Never ignore a significant finding silently,
  and do not repeat an argument without new evidence: when a disagreement does
  not converge, leave it to the owner.
- Merge only when CI is green. Pull requests are squash-merged, so each lands
  on `main` as one commit, and the branch is deleted after merging.
- Commit messages have an imperative subject of about 72 characters or fewer,
  and a body that explains why when that is not obvious.
- **Releases** are `vX.Y.Z` tags of Assemblr itself on `main`, created by
  the owner. Agents do not create or push release tags. They are unrelated to
  the `v<version>` tags that units use for publication.
- **Implementation progress is tracked on GitHub**, not in this repository.
  Follow the workflow in §13 with the configuration in §14.

## 13. Implementation workflow

These rules apply to any repository that tracks implementation through a
GitHub umbrella issue. The project configuration (§14) names the umbrella,
the owner, and the conventions. Sections 1–12 still apply in full. GitHub
tracks execution only: an issue, PR, or comment never outranks the approved
contracts and never defines domain behavior.

### 13.1 What a request authorizes

| Request | Authorizes |
|---|---|
| Status: "where are we", "what's blocking" | Establishing state (§13.4) and reporting it. Nothing else. |
| Continue: "next task", "resume" | Establishing state, then acting on the item that §13.5 selects, within its issue's scope. |
| "Initialize tracking" | Establishing state, then §13.3. |

When the request is ambiguous, inspect and report instead of acting.

### 13.2 Readiness

Before acting, confirm:

1. The repository, its remote, and its integration branch are identified, and
   its engineering instructions are loaded.
2. `gh` is authenticated with the permissions the operation needs: read for
   status, write for issues, branches, and PRs.
3. The local state is understood: the current branch, uncommitted changes,
   unpushed commits, and how each local branch relates to its remote. Never
   discard or overwrite them. If they belong to other work, stop and report.
4. **For tracking:** authoritative project documents (contracts,
   architecture, an approved plan) describe the intended work.
5. **For implementation**, in addition: the selected issue states its scope,
   exclusions, and acceptance criteria, and every product behavior it needs is
   approved. If not, it is blocked (§13.7).

If essential context is missing, stop and say what is needed. Do not invent
specifications, architecture, or tasks to fill the gap.

### 13.3 Initializing tracking

Only when explicitly requested.

1. Look for an existing tracker: the umbrella in §14 if one is configured,
   otherwise open and closed issues that act as umbrellas or roadmaps, issues
   with sub-issues, and references in the repository's documentation.
2. If exactly one valid tracker exists, reuse it. If several are plausible,
   ask which one. If none exists, create one umbrella that records the
   approved direction, plus issues for the first actionable increments only.
   No speculative backlog.
3. Record the umbrella in §14 through a pull request.

Running initialization again finds and reports the existing tracker. It
creates nothing.

### 13.4 Establishing state

No single command returns all of this; §14 lists the queries. Gather:

- **Local:** as in §13.2, plus local branches named after tracked issues.
- **Umbrella:** its body (trajectory, conventions, decisions) and its
  sub-issues with their state and order.
- **Each open increment:** its blocked-by relations, the intervention issues
  that affect it (§13.7), and comments since the last agent activity.
- **Open PRs:** draft status, linked issues (closing references and branch
  name), head commit, check results, the automated review, and review and
  owner comments.
- **Recently closed increments and PRs:** whether each issue closed through a
  merged PR. A closed issue or PR does not mean the work was completed.

Then reconcile. Each open increment has at most one branch and one open PR,
and each merged PR has closed its issue. Investigate any disagreement before
creating anything (§13.8). State in the report when GitHub was read: agents
inspect it when a session starts or when asked, and never monitor it
continuously.

### 13.5 Selecting work

Take the first case that applies.

1. An open PR has failed checks or actionable findings: fix them (§13.6).
2. An open increment has unfinished work, in a draft PR, a PR with unmet
   acceptance criteria, or a branch with no PR: resume it on that branch.
3. An open PR's checks or review are pending: report that and stop.
4. An open PR is waiting for the owner (approval, a decision, or the merge):
   report that and stop.
5. An intervention issue has a new response: process it (§13.7).
6. Otherwise take the first open increment, in the umbrella's order, whose
   blockers are closed and whose prerequisite PRs are merged.
7. If every listed increment is done, draft the next one from the umbrella's
   trajectory and the earlier PRs' findings, create its issue, and link it to
   the umbrella. If its actionable scope still needs confirmation, ask the
   owner to approve it and implement only after approval.

Never start an increment while a prerequisite PR is unmerged. Work on an
independent increment in parallel only when the owner authorizes it
explicitly and it shares no files or unmerged code with the other work.

### 13.6 Continuing an open pull request

Never open a second branch or PR for an issue that already has one.

| State | Recognized by | Action |
|---|---|---|
| In progress | Draft, or acceptance criteria unmet. | Resume on the existing branch. |
| Pending | Checks queued or running, or no completed review of the head commit. | Report and stop. A cancelled, skipped, failed, or older-commit review is not a completed one. |
| Actionable | Failed checks, unaddressed valid findings, or owner requests. | Fix within the issue's scope, reply with evidence to findings you believe are wrong, escalate domain decisions (§13.7), run §8, push, and re-check. Do not repeat an argument without new evidence. |
| Awaiting owner | Checks pass on the head commit, the latest review covers it, findings are addressed, and acceptance criteria are met. | Stop. Report that the PR is awaiting the owner and what the owner must do. Do not start dependent work. |
| Inconsistent | For example, a PR closed without merging, an issue closed without a merged PR, an open issue whose branch has no PR, or two PRs for one issue. | Investigate and report. Do not create replacement work. |

A merge accepts the merged change and any decision the owner explicitly
approved as part of the PR. It does not answer questions the PR asked
separately. Keep each unanswered question visible, in the next increment's
issue, an intervention issue, or the open-question register. Assess whether
it blocks a future increment, and never resolve it by assumption.

### 13.7 Human intervention

Escalate only for:

- a consequential product or domain decision that is missing;
- an external action that needs human access or authorization;
- a permission or environment problem the agent cannot resolve;
- a disagreement that the available evidence does not settle;
- a next step that would violate approved constraints.

Do not escalate ordinary engineering choices or speculative doubts.

**Reuse first.** A request may already live in the increment's issue, its PR,
an open intervention issue, the open-question register (§14), or another
active discussion. If it does, keep it there. Create an intervention issue only when
the blocker must be tracked across sessions, affects several increments, or
does not fit the existing discussion.

**Intervention issue.** An ordinary issue titled `[Owner decision] …` or
`[Owner action] …`, linked as a blocker of each affected issue (natively where
available, otherwise by reference). Its body states:

- the decision or action required;
- why it blocks, and which work it affects;
- evidence and alternatives;
- a recommendation, when one is justified;
- what counts as resolution.

Never ask for credentials or secrets on GitHub.

**On every resume**, classify each open intervention issue:

1. **Awaiting response:** no answer yet. Report it.
2. **Response received:** check that it answers the question and that the
   owner made it (see "Owner decisions" below). If not, ask a precise
   follow-up.
3. **Action required:** the decision is clear. Apply it: domain decisions
   update the approved contracts (§14) through a PR under §1 and §12, and
   other decisions update their documentation or configuration. For an
   external action, verify it was done, or ask the owner to confirm.
4. **Resolved:** the resolution criteria are met and verified. Close the
   issue with a link to the evidence and re-evaluate the work it blocked.

A comment alone does not resolve a blocker.

**Owner decisions.** When agents and the owner share a GitHub account, neither
the author of a post nor the absence of an attribution line shows who wrote
it. Agents mark what they post with an attribution line, but that is an
operational convention, not authentication. Act on a consequential decision
only when the owner has given it unambiguously, for example directly in the
session or by merging the change that records it. Never infer approval from
an ambiguous comment: ask the owner to confirm it explicitly, and treat it as
"response received" until then.

### 13.8 Concurrency and recovery

GitHub provides no locking for this workflow, so assume another session may
be working at the same time.

- Before creating an issue, branch, or PR, search for an existing one by
  title, branch name, and closing reference, and reuse it.
- Push a new branch and open a draft PR as soon as the first commit exists.
  The draft PR is the visible claim on the increment.
- If two sessions or duplicate resources target the same increment, continue
  with the earlier one where that is safe, delete nothing, and report the
  conflict to the owner.
- If a remote branch moved, fetch and reconcile. Never force-push (§12).
- Never discard uncommitted changes, overwrite another branch's work, rewrite
  pushed history, merge pull requests, or create duplicates to get around an
  inconsistent state.
- Stop when authentication, permissions, or context are insufficient. An
  explicit blocker is always preferable to an unsafe assumption.

## 14. Assemblr tracking configuration

- **Umbrella:** [#9](https://github.com/metamao/assemblr/issues/9). Its body
  holds the trajectory, the per-increment workflow, the owner-decision table,
  and the standing defaults. Read it there rather than copying it here.
- **Owner:** `metamao`, who approves, decides, and merges.
- **Increments:** sub-issues of #9, in trajectory order. Branches are named
  `feat/<issue>-<slug>`, for example `feat/6-unit-manifest`. The PR says
  `Closes #N`, lists the checks it ran and any changes to `docs/domain/`,
  ADRs, or open contracts, and ends with "Findings for later increments".
- **Contracts:** approved contracts live in [`docs/domain/`](docs/domain/),
  and open questions in the register
  [`open-contracts.md`](docs/domain/open-contracts.md).
- **Interventions:** domain questions belong in the register. A question that
  blocks the next increment gets an `[Owner decision] Decide OC-<n>: …` issue.
- **Checks:** [`ci.yml`](.github/workflows/ci.yml) runs format, lint, build,
  and test on Linux and Windows, and checks the MSRV.
- **Automated review:** [`claude-review.yml`](.github/workflows/claude-review.yml)
  reviews non-draft PRs when they are opened, pushed, or marked ready, and a
  new push cancels the previous run. It posts as `claude`: inline findings
  plus one top-level comment starting `Reviewed <7-character SHA>`. A review
  counts only if that SHA matches the PR's head commit. On a non-draft PR, a
  missing review means pending, cancelled, or failed, never clean. Its note
  that changes to `AGENTS.md` or `docs/domain/` need the owner's approval is
  expected and means the PR is awaiting the owner, not that it needs a fix.
- **Shared account:** agents run with the owner's `gh` credentials, so GitHub
  shows their issues, PRs, and comments as written by `metamao`. Agents end
  what they post with an attribution line such as
  `🤖 Generated with Claude Code`. Earlier posts carry none. Owner decisions
  follow §13.7.
- **Queries:**

  ```sh
  gh issue view 9
  gh api 'repos/{owner}/{repo}/issues/9/sub_issues' --jq '.[] | {number, state, title}'
  gh api 'repos/{owner}/{repo}/issues/<n>/dependencies/blocked_by' --jq '.[] | {number, state, title}'
  gh issue list --state open --search 'in:title "[Owner"'
  gh pr list --state all --json number,title,state,isDraft,headRefName,closingIssuesReferences
  gh pr view <n> --json headRefOid,isDraft,statusCheckRollup,closingIssuesReferences,comments,reviews
  gh api 'repos/{owner}/{repo}/pulls/<n>/comments' --paginate
  ```

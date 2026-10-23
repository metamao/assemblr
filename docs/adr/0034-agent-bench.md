# 0034: The agent bench: coding agents on fixed scenarios, run by hand before a release

- **Status:** Accepted
- **Date:** 2026-10-23

## Context

Coding agents are first-class users of Assemblr (AGENTS.md §1), and
whether they use it well depends on text that tests cannot judge: the
guide that `assemblr guide` prints, `--help`, hints, and the pointer
installed for the agent. The research of 2026-10-23 (#105) measured this
with headless Claude Code runs on a sample registry: without any mention of
Assemblr, agents did not use it (0/9); with a two-line pointer to
`assemblr guide`, they did (24/24); one misleading example in the guide
caused errors in 2 of 3 runs. A guide can also stay true and still stop
describing the best way to work, which the unit test that checks its
command names does not catch.

Those runs used scripts outside the repository. The owner decided
(2026-10-23) that the bench lives in the repository, runs by hand and not in
CI, before releases and on pull requests that change the guide or the
commands it describes, at a cost of about 3–5 USD per full run.

## Decision

1. **`cargo agent-bench` is a Cargo alias for `examples/agent_bench/`**, like
   `cargo sandbox` (ADR 0028): Rust, the crate's existing dependencies, and
   nothing else from the repository's toolchain. It needs Claude Code
   (`claude`) on `PATH`, and it supports Linux and macOS, where Claude Code
   runs its shell commands in a Unix shell.
2. **A scenario is a starting state, a prompt, and checks of the end state.**
   The fixture is small but real: nine sample units in
   `examples/agent_bench/fixture/`, published, registered, and composed with
   the binary under test, plus later releases, a template project, and a
   catalog where a scenario needs them. Checks look only at the end state (a
   unit in the lock, a route in the code, a clean catalog commit), never at
   the commands the agent chose. The ten scenarios are those of the
   research, with its Spanish prompts, so that results compare with it.
3. **Each run is isolated.** It gets a fresh directory under the system's
   temporary directory, outside any repository, with homes of its own (so
   its own registry, Git identity, and Claude Code configuration), and
   commands run with a cleared environment: only the variables Claude Code
   needs to reach its API pass through. Claude Code therefore authenticates
   with `ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN`, never with the
   caller's login. The agent may use file tools and a fixed list of shell
   commands without asking.
4. **A condition sets what the agent knows beforehand:** `none`, `hint` (the
   prompt says that Assemblr is installed), `pointer` (the user-level
   `CLAUDE.md` holds the pointer to `assemblr guide`), or `skill` (the binary
   under test ran `assemblr skill install claude`). `skill` is the default,
   as what users get.
5. **The report is data for a person, not a gate.** Each run keeps its
   prompt, transcript, and checks under `target/agent-bench/<run id>/`, and
   `summary.md` gives, for each scenario and condition, the runs that passed
   every check, the checks passed, the runs that read the guide, the
   `assemblr` commands and how many failed, and the cost, then lists the runs
   to look at: failed checks, subcommands that do not exist, reads of
   `.assemblr/` or the registry, and permission denials. The exit status says
   only whether the bench could run.
6. **`tests/agent_bench.rs` keeps the bench working without an agent.** It
   builds every scenario's starting state with the binary under test and
   requires each scenario to fail a check there, so a CLI change that breaks
   a scenario fails CI, and a pass in the bench always means the agent did
   the work. It also tests how transcripts are read. It runs on Unix only,
   like the bench.

## Consequences

- A change to the guide, a command it describes, or a hint agents follow is
  checked by running the bench on the scenarios it affects, before the
  merge. A release runs all of them.
- Results vary between runs and models, so the bench compares rates over
  several runs (`--runs`, two by default), not single outcomes.
- The bench is a development tool, not part of the product: it is not
  shipped, and the binary knows nothing about it.
- The scenarios cover what the research measured. A new workflow gets a
  scenario in the change that adds it, when agents are expected to use it.

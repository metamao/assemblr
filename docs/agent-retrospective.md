# Agent retrospectives

A retrospective records how well this repository's development environment
supported a task: the instructions in `AGENTS.md` and `CLAUDE.md`, the
contracts and ADRs, the tools and skills available, the GitHub workflow, and
the automated review. It is not about the quality of the implementation, and
it changes nothing: it makes no implementation decision, opens no issue,
triggers no improvement, and never blocks delivery. Entries accumulate in
`.retro/`, which is Git-ignored, for a separate analysis across many runs.

## When

Write one entry at the end of a session that completed a substantive
implementation task: an increment's PR opened or pushed with meaningful
changes, a defect fix, or a change to the engineering workflow or
documentation of comparable size. Write it after the work is pushed and
before the final report, so the observations are fresh and the entry cannot
delay delivery.

Do not write one for a status request, a resumed session that only
re-established state, a review round that changed a few lines, an
investigation that ended in an intervention issue, or an automated review
run.

## How

- Create the file with the **Write tool**, which creates `.retro/` when it is
  missing. Do not use shell redirection or a script, and do not list, read,
  search, or summarize existing entries: `.retro/` is write-only for agents so
  that every entry is independent of the earlier ones.
- Name it `<UTC timestamp>-<task>.md`, for example
  `2026-10-21T0315Z-pr-35.md`, where `<task>` is the PR or issue number or a
  short slug. The timestamp keeps names unique without reading the directory.
- Start with one header line: the date, the task (PR or issue), the model or
  agent, and the kind of session (new increment, resume, fix, workflow).
- Then 50–150 words of candid, concrete observations in whatever shape fits.
  Name the instruction, document, tool, or step you mean. The prompts below
  are for reflection, not a questionnaire: say what stood out, and if nothing
  did, one sentence saying so is a complete entry.

## Prompts

- Which instruction, document, tool, or workflow step saved time or prevented
  a mistake?
- Where did you lose time: unclear or contradictory instructions, missing
  documentation, tooling friction, or workflow steps that added no value?
- What capability, information, or convention was missing?
- Did the decision tiers (AGENTS.md §1.1), the decision log, escalation, or
  the automated review fit the situation, or did they get in the way?
- What one change to this environment would have helped most?

## Example

Illustrative only; the PR, the observations, and the agent are invented.

```markdown
# 2026-11-03 · PR #NN · <model or agent> · new increment

The ADR index and the "Findings for later increments" of the previous PR
gave me the seams for the command in minutes. I lost about an hour because
the register said a detail needed the owner while the issue said to proceed;
§1.1 would have settled that. The fault-injection suite takes long enough
that I skipped it between small edits. Missing: a way to run only the
integration tests of one command. The review found a real recovery bug on
the first pass, which the acceptance criteria did not cover.
```

## Isolation of `.retro/`

The write-only rule is a convention for agents, not an enforced boundary,
and the owner has decided not to enforce it for now. Two Claude Code
mechanisms were tested (Claude Code 2.1.306): a `Read(./.retro/**)` deny
rule also blocks the `Write` and `Edit` tools on those paths, so no entry
could be created; a `PreToolUse` hook that denies `Read`, `Glob`, `Grep`,
and `Bash` calls naming `.retro` lets `Write` and `Edit` through but does
not cover indirect access such as `find`, `grep -r`, or scripts. Neither is
installed. Revisit only with a concrete need and a reliable mechanism.

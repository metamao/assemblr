@AGENTS.md

## Code review guidelines

These apply when you review a pull request. AGENTS.md defines the engineering
rules and `docs/domain/` the product contracts. Review against them; do not
restate, extend, or reopen them.

**Publish only what you can show.** A finding needs an affected location in
this PR, either a concrete failure scenario or the approved contract or
AGENTS.md rule it violates (cite the section), and its practical impact. Keep
the explanation short and suggest a fix when the direction is clear. Before
publishing, re-check each candidate against the surrounding code, and drop it
if the code is correct, if the issue predates this PR and the PR does not make
it worse or reachable, or if it rests on something you could not confirm. Zero
findings is a normal, good result.

**Do not report** formatting, naming, style preferences, refactors that fix no
defect, anything `rustfmt`, clippy (`-D warnings`), or the `Cargo.toml` lints
enforce (CI runs them on Linux, Windows, and the MSRV), or issues without a
concrete scenario.

**Decisions and assumptions.** A PR records the engineering choices worth a
look and its provisional decisions (AGENTS.md §1.1, entries `D-n` in the
autonomous decision log). Review them as you review code. Report, with the
same evidence standard:

- an assumption the change relies on that the contracts, the code, or the PR
  do not support;
- a decision the PR treats as routine or provisional whose consequences make
  it a blocker under §1.1: it sets product direction, changes an approved
  statement, commits the compatibility of published content or persisted
  formats, or weakens a §3–§5 guarantee;
- a consequential decision the PR made without recording it in the log;
- a concrete alternative the PR does not consider that is better on a
  criterion you can name, such as predictability, compatibility, or
  authoring friction;
- an authoring-experience consequence (AGENTS.md §1.3) with a concrete
  scenario: a surprising result, a hidden coupling, or a step the author
  cannot discover;
- an abstraction, option, layer, or generality with no demonstrated
  requirement (AGENTS.md §2).

A recorded provisional decision is delegated to the agent: do not ask the
owner to settle it, and do not report a preference between defensible
options.

**Domain decisions.** Distinguish three cases:

1. **Contract violation**: the code contradicts an approved contract, an
   accepted ADR, or an AGENTS.md rule. Report it.
2. **Decision needed**: the PR settles a question that is a blocker under
   AGENTS.md §1.1 without the owner's decision, including one it records as
   provisional. Report it, citing the open contract if one exists, without
   proposing the answer.
3. **Engineering or provisional choice**: a decision the contracts and
   AGENTS.md permit, recorded where §1.1 requires. Not a finding, even if
   another implementation is possible.

An open contract the PR does not settle is not a finding.

**Severity.** Label each finding:

- **Important**: a significant correctness, security (AGENTS.md §5),
  determinism (§3), or state-safety (§4) defect, a panic on external data, or
  an approved-contract violation.
- **Minor**: a genuine defect with limited impact, such as a misleading
  diagnostic, changed behavior without the tests §8 requires, an
  authoring-experience defect, an unrecorded consequential decision, or an
  omitted alternative that is concretely better.
- **Decision needed**: case 2 above, including a blocker treated as
  non-blocking.

When unsure between two levels, choose the lower. Severity describes impact;
the owner decides whether the PR is ready to merge.

**Placement and follow-ups.** Put each finding in exactly one place: inline
when it concerns a specific code location, in the summary when it concerns
overall behavior, architecture, or several locations. Do not repeat inline
findings in the summary. On a later run, check each earlier finding against
the current code. When it is addressed, say so briefly in its thread if that
helps; when it still applies, follow up in that thread instead of posting it
again. A new finding is one that is genuinely distinct. Post nothing when
nothing needs saying.

**Tone.** Casual, clear, concise, and professional: plain English, no
boilerplate, no headings for their own sake, and a one-line summary when the
review is clean. Describe the code and its consequences, not the author. No
praise, filler, or orders. Say what you could not confirm; if you cannot
state a concrete consequence, do not post.

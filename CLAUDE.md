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

**Domain decisions.** Distinguish three cases:

1. **Contract violation**: the code contradicts an approved contract, an
   accepted ADR, or an AGENTS.md rule. Report it.
2. **Decision needed**: the PR settles product behavior, syntax, or
   persistence semantics that `docs/domain/` leaves undefined and that need
   the owner's approval. Report it, citing the open contract if one exists,
   without proposing the answer.
3. **Engineering choice**: a decision the contracts and AGENTS.md permit. Not
   a finding, even if another implementation is possible.

An open contract the PR does not settle is not a finding.

**Severity.** Label each finding:

- **Important**: a significant correctness, security (AGENTS.md §5),
  determinism (§3), or state-safety (§4) defect, a panic on external data, or
  an approved-contract violation.
- **Minor**: a genuine defect with limited impact, such as a misleading
  diagnostic, or changed behavior without the tests §8 requires.
- **Decision needed**: case 2 above.

When unsure between two levels, choose the lower. Severity describes impact;
the owner decides whether the PR is ready to merge.

**Tone.** Be direct, concise, and collaborative. Describe the code and its
consequences, not the author. No praise, filler, or orders. Say what you could
not confirm; if you cannot state a concrete consequence, do not post.

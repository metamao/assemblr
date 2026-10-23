# User-facing copy

How Assemblr writes what its users read: `--help`, errors, notes, hints,
success output, the README, and the agent guide. The goal is text that a busy
person or an agent can read once and act on. [AGENTS.md §6](../AGENTS.md#6-errors-and-diagnostics)
defines what a diagnostic must contain; this file defines how it reads.

## Rules

1. **Write for someone who has not read `docs/domain/`.** Use plain words
   and sentences of 20 words or fewer, 25 at most. One idea per sentence.
2. **Use one term per concept, from the user glossary:** unit, project,
   registry, version, requirement, option, parameter, amendment, conflict,
   build, lockfile. Internal terms stay internal: reconciliation base,
   explicit selection, the root, requirer, declared composition. An argument
   that names a unit is a "unit" (`<UNIT>`), never an "identity".
3. **Write errors in three parts, as rustc and Git do.**
   - `error:` what failed, with the concrete value, without the fix.
   - `note:` the state or context, such as `nothing was changed`.
   - `hint:` the action, with a command that can be copied.

   Start with a lowercase letter and end without a period. No "please", no
   apologies, and no "did you mean?": state the suggestion.
4. **Say each thing once.** One cause gives one error, and the hint does not
   repeat the error.
5. **Keep success output short.** One line with the result, details as an
   indented list, and the next step only when it is not obvious. Hashes and
   internal details appear only with `--verbose` or `--json`. Use correct
   plurals, never `file(s)`.
6. **Layer the help.** A command's summary is an imperative line of at most
   60 characters with no final period; it is what `-h` and the command list
   show. `--help` adds at most three sentences and two or three examples.
   Each flag gets one sentence. Placeholders are uniform: `<UNIT>`, with the
   example `@acme/web`.
7. **Make the README a guide, not a specification.** What Assemblr is, how
   to install it, a first use for consumers and for authors, the key
   concepts, and links. Development comes last.
8. **Use US English, the Oxford comma, and backticks** for commands, code,
   and paths.
9. **Text is not a contract.** Agents should rely on `--json` and exit codes.
   Tests assert on stable substrings, as AGENTS.md §8 requires, so wording
   can improve without breaking them.

A unit test in `src/cli/mod.rs` checks rule 6 for every command summary.

## Examples

| Case | Before | After |
| --- | --- | --- |
| `compose` summary | Compose the consumer project in the current directory from the units its assemblr.jsonc declares, or settle a composition that left conflicts in files with --continue or --abort | Compose the project from assemblr.jsonc |
| Below the project | `error: …/app/sub/assemblr.jsonc does not exist` and a hint suggesting `assemblr init` | `error: no assemblr.jsonc in …/app/sub`, `note: nothing was changed`, `hint: the project is in …/app; run the command there` |
| A required value | `error: required parameter appName of @acme/base 1.0.0 has no value and no default; it must be supplied by the root` | `error: @acme/base 1.0.0 needs a value for appName, which has no default; …`, `hint: run assemblr add @acme/base --option appName=<value>, …` |
| Compose result | `Reconciled the project with 1 unit (2 file(s)): created 2, modified 0, merged 0, removed 0; the reconciliation base sha256-v1:… is stored in .assemblr/.` | `Composed 1 unit into 2 files: 2 created.` |

## Sources

- [rustc diagnostics](https://rustc-dev-guide.rust-lang.org/diagnostics.html):
  the error, note, and help structure; no questions.
- [Google, writing helpful error messages](https://developers.google.com/tech-writing/error-messages):
  the cause, the fix, and consistent terms.
- [GOV.UK, sentence length](https://insidegovuk.blog.gov.uk/2014/08/04/sentence-length-why-25-words-is-our-limit/):
  the 25-word limit.
- [Command Line Interface Guidelines](https://clig.dev/): examples first,
  the important part last.

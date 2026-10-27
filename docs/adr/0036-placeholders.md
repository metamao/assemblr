# 0036: Placeholders: no rendering, the author's checks, and the consumer's report

- **Status:** Accepted
- **Date:** 2026-10-27

## Context

The owner replaced parameters, options, and templates with declared
placeholders on [#124](https://github.com/metamao/assemblr/issues/124):
a unit's files hold literal tokens such as `__PROJECT_NAME__`, the manifest
declares them, Assemblr delivers every file byte for byte, and the consumer
replaces them (see [placeholders.md](../domain/placeholders.md)). The
renderer of ADR 0009, the mappings and examples of ADR 0025, and the filters
and patterns of ADR 0035 implemented the model that #124 removed. This ADR
records how the replacement is built and what it supersedes.

## Decision

1. **Nothing is rendered.** `adapters::render`, `domain::parameter`, the
   option values of the manifest and of the project declaration, and every
   step that resolved values from dependents to dependencies are removed.
   A composed file's content is the provider's bytes, or the composition of
   its amendments on them, at every point where a file used to be rendered:
   builds, consumer compositions, layered amendments (ADR 0032), cross-version
   application (ADR 0023), and `unit update`'s rebase. Every other ADR's
   mention of rendering or templates reads, from now on, as the files as they
   are. There is no compatibility mode: the reader rejects `params` and
   `options` with an error that names the replacement (`RemovedField`), so a
   manifest or a declaration written for the old model fails with advice
   rather than as an unknown field.
2. **The domain owns the token rules.** `domain::placeholder` holds
   `PlaceholderName` (the form, with a suggestion for a near miss),
   `Placeholders` (the manifest's `replace` and `ignore`), and two pure
   functions over data. `check_placeholders` takes the unit's manifest, its
   `src/` and `amendments/` trees, and the manifests of its resolved graph,
   and returns every problem and the unresolved tokens, sorted.
   `PlaceholderReport::find` takes `(path, bytes, declarations)` triples and
   returns, per placeholder and declaring unit, the description and every
   location. Neither reads a file. Only UTF-8 text is scanned, by the merge's own
   test of what is text; other bytes are skipped, as the contract says. Lines are counted by `\n` and columns
   in characters, so CRLF and non-ASCII text give an editor's positions.
3. **Two matching rules, on purpose.** An author's undeclared token is found
   identifier-bounded, so `MY__X__` is not a token, while a declared
   placeholder is found by substring, in the unit's files (for `Unused`) and
   in the project alike, so `__NAME___db` holds `__NAME__`. A bounded search
   finds fewer false warnings in the author's files; a substring search
   never misses a value the consumer has to replace.
4. **The checks run in the build's validation, after resolution.**
   `adapters::build::validate` calls `check_placeholders` once the graph is
   resolved and before composition, so `unit build`, `unit check`,
   `unit publish`, and `unit add` share it. Its problems are
   `BuildProblem::Placeholder` and gate the build; its unresolved tokens are
   returned beside the result, a warning in `unit build` and a
   `CheckProblem::Undeclared` in `check` and `publish`. A resolution failure
   reports no placeholder problem, since the graph's declarations are not
   known.
5. **File scope travels with the composition.** Each composed file carries
   the declarations in its scope: those of its provider and its amenders and
   of the units each of them depends on, sorted by placeholder and unit. The consumer
   composition keeps the scope of each file whose composed content holds a
   declared placeholder, and the report searches only those paths: in the
   planned files for what the composition writes, and in the project's
   current content for what it keeps. `status` plans the composition
   exactly as `compose` does and reads only managed files, so it needs no
   stored state and agrees with `compose` by construction
   (**Provisional (D-50)**).
6. **The CLI formats one report.** `cli::placeholders` prints the body
   shared by `compose`, `add`, `update`, `remove`, and `status`, and the JSON
   shared by `status --json`; `report()` in `cli::project` prints it after
   the summary on stdout and the hint on stderr. `unit build` lists each
   placeholder of the composed result with its file count. `info` reads the
   declarations from the indexed manifest, which the catalog index embeds
   already (ADR 0030), so the index format does not change.
7. **Dependencies.** `regex-lite` moves back to the development
   dependencies: only the schema tests compile patterns now. The placeholder
   form is matched by hand in `domain::placeholder`, and the schema test
   checks the schema's pattern against `PlaceholderName::parse` (ADR 0014).

## Consequences

- ADRs 0009, 0025, and 0035 are superseded. Point 2 of ADR 0010 (the
  renderer's `own`), the option values of points 1 and 2 of ADR 0016, and
  the parameter rules of the schema in points 4 and 5 of ADR 0014 no longer
  exist; point 6 of ADR 0023 (rendering across provider versions) is moot,
  since both sides are the files as they are, and with it the check of
  parameters a template names.
- Several thousand lines of rendering, value resolution, and their tests go,
  and a file's composed bytes no longer depend on any value.
- A placeholder in a JSON key or a path is the author's risk, warned about
  in the guidance, not checked beyond paths: a check on keys would need a
  parser per format for a rule the contract states as guidance.
- The report reads only the managed files that can hold a placeholder, so
  its cost grows with those files, not with the project.

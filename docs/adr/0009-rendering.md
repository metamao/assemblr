# 0009: Rendering templates with a purpose-built interpolator

- **Status:** Accepted
- **Date:** 2026-10-21

## Context

`unit build` renders a unit's templates with its parameter values. The
product rules are in
[`parameters-and-options.md`](../domain/parameters-and-options.md#rendering--approved)
and [`units.md`](../domain/units.md#templates): templates are the `src/`
files ending in `.assemblr.tmpl`, the grammar is `{{ name }}` references,
`{{ own.name }}`, and the escape `{{ "{{" }}`, every other construct is an
error, and a rendered file is a pure function of its source and its
context. This ADR records how that is implemented, and why no template
engine crate is used.

The owner decided the grammar deliberately narrow for Alpha (#24). The
amendments model approved in #23 adds one requirement: amendment copies,
rendered in a later increment, will see an additional `own` context, and
they must use the same template language.

## Decision

1. **No engine crate.** The grammar has three forms, none of which needs an
   expression language. A Jinja engine would bring control flow, filters,
   and whitespace control that the contract forbids, a configuration surface
   to lock down (loaders, autoescape, undefined handling, trailing-newline
   stripping), and a dependency to track against the MSRV. ADR 0002 adds a
   crate only for immediate value, and there is none here. If the owner
   ever approves constructs beyond references (OC-5), the grammar is a
   strict subset of Jinja's, so an engine could render today's templates
   identically; the escape is the Jinja string literal `"{{"` for that
   reason.
2. **Where the code lives.** The renderer is `adapters::render`. ADR 0001
   assigns rendering to the adapters, as a capability the application uses;
   this one happens to need no I/O and no dependency, which keeps it a pure
   function and lets its tests run without a file system. Template
   *selection* is domain code, in `domain::composition`: the delivered path
   decides collisions, which are a composition rule. `adapters::build`
   renders after composition and before publication, so a rendering error
   rejects the build before `build/` or the lockfile change (ADR 0008).
3. **Selection.** A file is a template when its path ends in
   `.assemblr.tmpl`, compared exactly and case-sensitively. The suffix
   contains no `/`, so matching the whole path matches the last segment.
   The delivered path is the source path without the suffix, re-validated
   with the same `RelativePath` rules as a source path: a bare
   `.assemblr.tmpl` leaves an empty name, and `notes..assemblr.tmpl` a name
   ending in a dot, both rejected with the existing path diagnostics. The
   composed tree and the collision key are built on delivered paths, and a
   collision lists source paths, with the delivered path when it differs.
   `ComposedFile` keeps the source path and whether the file is a template.
4. **Scanning.** The renderer makes one pass over the source. It copies text
   up to the next `{{`, `{%`, or `{#`, finds that opener's closer (`}}`,
   `%}`, `#}`), and classifies the construct:
   - an expression whose trimmed inside is `"{{"` produces `{{`;
   - an inside that parses as a parameter name is a reference; `own.` plus a
     parameter name is an `own` reference;
   - anything else, and every statement or comment, is an unsupported
     construct, reported with its full text;
   - an opener with no closer, or with another opener before its closer
     (other than the escape's own `{{`), is unterminated; scanning resumes
     right after it, so the next construct is reported on its own.

   Inside an expression, "whitespace" is ASCII whitespace, including line
   breaks. A `}}` with no opener is text. Inserted values go straight to
   the output, which is never rescanned, so a value containing `{{ x }}`
   renders as that text. Text between constructs is copied byte for byte,
   including `\r\n` and the final newline or its absence.
5. **Contexts.** A `Context` holds the providing unit's resolved values,
   which `Composition::values()` already computes for every unit. A
   reference resolves against it; an `own` reference resolves to nothing in
   a unit's own files and is reported as undefined with a message that says
   where `own` is defined. The amendment renderer will give `Context` an
   `own` set without changing the grammar or the scanner. Nothing is added
   for that now.
6. **Errors.** Every construct that fails is an error with the line and
   column of its first character: lines count `\n` from 1, and columns count
   characters from 1, as the manifest reader reports positions. A file's
   errors come in position order; `unit build` reports every template's
   errors in delivered-path order, as one build problem each, and rejects
   the build. A template that is not UTF-8 is one error for the file.
7. **Bounds.** A rendered file may not exceed ADR 0006's per-file limit,
   16 MiB, which `FILE_LIMIT` already bounds a source file to. A value that
   would take the output past it is an error at that reference. Rendering
   is linear in the source and the values; there is no recursion.
8. **Values.** A string is inserted unchanged, an integer as its decimal
   digits, a boolean as `true` or `false`. No quoting or format escaping
   exists, by decision; a filter system would be the place for it and is
   out of scope (OC-5).
9. **`own` as a name.** `ParameterName::parse` rejects exactly `own`, with
   its own reason, so manifests cannot declare it. Other spellings, such as
   `owner` or `Own`, are ordinary names.

## Consequences

- Units with parameters now build differently: templates are rendered, and
  a `{{` outside a template is content. A template author who needs a
  literal `{%` or `{#` has no escape for it; `{{ "{{" }}` covers only `{{`.
  That is an OC-5 detail left for the owner if the need arises.
- A unit that provides `a.json` and `a.json.assemblr.tmpl` fails with a path
  collision, as two units would.
- Amendment composition (I8) renders a copy with the provider's values plus
  `own`, through the same renderer, by extending `Context`. Re-basing
  across provider versions needs the context rule OC-52 leaves open.
- Should the owner approve a richer grammar, the module is replaced, not
  grown: a Jinja engine with loaders, autoescape, and whitespace control
  disabled, strict undefined, and the trailing newline kept would be the
  natural candidate, and it must be checked against the MSRV and against
  the byte-for-byte rule before adoption.

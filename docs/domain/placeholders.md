# Placeholders

A unit's files hold the values a project must set, such as its name, as
**placeholders**: literal tokens the consumer replaces in their own files.
Assemblr never substitutes anything. It delivers every file byte for byte,
checks what the unit's author declares, and tells the consumer which
placeholders remain (owner decision of 2026-10-27,
[#124](https://github.com/metamao/assemblr/issues/124)).

This replaces the earlier parameters, options, and templates: a unit has no
configuration inputs, and a project's declaration holds no values.

## Form — Approved

A placeholder is two underscores, an uppercase ASCII letter, any number of
uppercase ASCII letters, digits, or underscores, and two underscores:
`__PROJECT_NAME__`, `__API_URL__`. Nothing else is a placeholder.

## Declaration — Approved

A unit declares its placeholders in `manifest.jsonc`:

```jsonc
"placeholders": {
  "replace": {
    "__PROJECT_NAME__": "The project's name in kebab-case. It becomes the npm package and Worker name.",
    "__APP_TITLE__": "The application's visible name."
  },
  "ignore": ["__VERSION__"]
}
```

- `replace` maps each placeholder the consumer replaces to a non-empty
  description of the value to write.
- `ignore` lists tokens of the placeholder form in the unit's files that are
  not placeholders, such as a bundler's define.

Both are optional, and `placeholders` itself is optional. A key of `replace`
or an entry of `ignore` that does not have the form is a manifest error that
suggests the form, for example `__project-name__` → `__PROJECT_NAME__`.

A dependency's placeholder is declared once, by the dependency: a unit that
writes it in its own files, or keeps it in an amendment copy, uses that
declaration and its description.

## Author checks — Approved

`unit build`, `unit check`, and `unit publish` check a unit's placeholders
against its own files (`src/` and `amendments/`) and its resolved graph.
Only UTF-8 text files are inspected; other files are delivered as they are,
and a declaration found only in them counts as unused.

These are errors, all reported together, and nothing is built or published
with any of them:

- a path in `src/` or `amendments/` that holds a placeholder the unit or a
  unit of its resolved graph declares in `replace`: Assemblr never renames a
  file, so a placeholder belongs in a file's content;
- a placeholder in `replace` that a unit of the resolved graph declares
  already, naming that unit: each placeholder has one declaration and one
  description;
- a token in `ignore` that a unit of the resolved graph declares in
  `replace`: it would hide the placeholder from consumers;
- an entry of `replace` or `ignore` that no file of the unit contains.

A token of the placeholder form in the unit's files, delimited as an
identifier (by characters other than ASCII letters, digits, and `_`), is
**resolved** when the unit or a unit of its resolved graph declares it in
`replace` or lists it in `ignore`. An unresolved token is a warning in
`unit build`, which still builds, and an error in `unit check` and
`unit publish`. The hint offers both fixes.

The build summary lists each placeholder the composed result holds and in
how many files, in name order.

## Consumer report — Approved

After `compose`, `add`, `update`, and `remove`, and in `status`, Assemblr
reports the declared placeholders that remain in the project's managed
files, with each declaration's description and every location. The report
is informational: it never stops a composition and never changes an exit
code. It is hidden while a composition waits for the consumer or was
interrupted, as `status` hides edits then.

- Placeholders are found in the composition Assemblr produces, before it
  writes it, and in the managed files it already reads. No file Assemblr does
  not manage is ever read.
- **File-scoped attribution:** an occurrence in a file is interpreted with
  the declarations of the units that contribute to the file, its provider
  and its amenders, and of their resolved graphs, the same scope as the
  author's checks. A token that an unrelated unit declares is not reported
  in that file, and a unit that lists a token in `ignore` hides it only in
  the files it contributes to. A file whose scope holds two declarations of
  one token is listed under both.
- In the project, a declared placeholder is found by exact text search, so
  `__PROJECT_NAME___db` and `api-__PROJECT_NAME__` hold it.
- The report is computed on each file's final content: an amendment that
  replaces a dependency's placeholder takes it out of that file, which is how
  a unit fixes a dependency's value; a consumer's replacement takes it out
  too. A later version that brings a new occurrence reports it again.
- Lines are counted by `\n`, as editors number them, and columns count
  characters. Placeholders are sorted by name, declarations by unit, and
  locations by path, line, and column.

### Report details — Provisional (D-50)

- `status` derives the report from the composition it plans and the managed
  files it reads already, with no stored state.
- A managed file is reported only when its composed content holds a
  declared placeholder in its scope. A placeholder the consumer writes into
  a managed file whose composed content holds none is not reported, so that
  `compose` and `status` always agree.
- The text report lists one `path:line` per row, a line that holds the
  placeholder twice once; `--json` lists every column.

## Output — Approved

`compose`, `add`, `update`, and `remove` print, after their summary and only
when a placeholder remains:

```text
2 placeholders to replace, in 3 files:

  __APP_NAME__

    @acme/app: The application's name, in kebab-case.
      README.md:1
      package.json:2

  __ROUTE__

    @acme/worker: The route the worker serves.
      wrangler.jsonc:3

hint: replace each placeholder with your value; `assemblr status` shows what remains
```

The list goes to stdout and the hint to stderr. `status` prints the same
body under `Placeholders to replace:`, without the hint. `status --json`
holds `placeholders`, an empty array when none remains and `null` while a
composition waits or was interrupted:

```json
"placeholders": [
  {
    "placeholder": "__ROUTE__",
    "declarations": [
      {
        "unit": "@acme/worker",
        "description": "The route the worker serves.",
        "locations": [{ "path": "wrangler.jsonc", "line": 3, "column": 13 }]
      }
    ]
  }
]
```

`line` and `column` are 1-based. `assemblr info <unit>` lists the unit's
own `replace` declarations and `ignore` tokens, in text and in `--json`
(see [registries-and-discovery.md](registries-and-discovery.md)).

## Authoring guidance — Approved

- Put a placeholder the consumer replaces in a value, never in a JSON or
  JSONC key or a file path: a value the consumer edits in a key conflicts on
  the next update that touches the object. In a text file, an upstream change
  next to an edited line is an occasional small conflict.
- Renaming or removing a placeholder in a new version is a breaking change
  for consumers: every line they already edited conflicts.
- Whether a unit's own tooling works with its placeholders in place is the
  author's concern, not Assemblr's.

## Removed

`params`, `options`, mappings, inherited parameters, examples, filters,
`pattern`, the `own` template context, templates and the `.assemblr.tmpl`
suffix, a project's `options`, and `add --option` no longer exist. A
manifest or a project declaration that still holds `params` or `options` is
rejected with an error that explains the replacement. There is no
compatibility mode.

## Deferred

- A way for the consumer to silence a placeholder they keep on purpose.
- A `unit publish` note when a new version removes or renames a placeholder.

# 0035: Template filters and parameter patterns

- **Status:** Accepted
- **Date:** 2026-10-25

## Context

Decomposing a real stack into units (#120) showed that a parameter value
with a quote, such as the title `Bob's "App"`, breaks every JSON, YAML, or
HTML file a template inserts it into, and that nothing let a unit say which
values it accepts. ADR 0009 rendered values unchanged by decision and left
filters to OC-5. The owner decided on #120: two filters with Jinja's output,
`tojson` and `e`, and a JSON Schema-style `pattern` on string parameters,
with a regex crate recorded in an ADR. The details are D-47 and D-48.

## Decision

1. **Filters in the purpose-built renderer.** ADR 0009 point 1 stands: no
   engine crate. A reference may carry one filter, `{{ name | tojson }}` or
   `{{ name | e }}`, on `name` or `own.name`; whitespace around `|` is free.
   The scanner of point 4 is unchanged; classification splits the inside at
   the first `|`. A known filter name with anything else (a chain, an
   argument) is an unsupported construct; an unknown name is its own error,
   `unknown filter`, that lists the two. This amends ADR 0009 points 4
   and 8.
2. **Output is Jinja's, byte for byte.** `tojson` is Jinja 3.1's: Python's
   `json.dumps` with ASCII output (`\uXXXX`, lowercase, surrogate pairs above
   U+FFFF), then `<`, `>`, `&`, `'` as `\u003c`, `\u003e`, `\u0026`,
   `\u0027`. `e` is MarkupSafe's: `&amp;`, `&lt;`, `&gt;`, `&#34;`, `&#39;`.
   The expectations in the tests were produced with Jinja 3.1.6. So ADR
   0009's promise holds: an engine could replace the module and render
   today's templates identically.
3. **One divergence.** A boolean renders `true` or `false` with no filter or
   `e`, as it did before filters; Jinja would write `True`. Integers render
   as digits everywhere. `tojson` of either writes the JSON literal, as
   Jinja does.
4. **`pattern` is a domain type.** `ValuePattern` keeps the source text and
   the compiled expression; two patterns are equal when their source is. A
   `ParameterDefinition` takes one only for `string`, and refuses a default
   or example that does not match. `check`, which every value path already
   goes through (options, mappings, `add --option`, development values),
   rejects a non-matching value with `NoMatch`, naming the value and the
   pattern. Matching is unanchored, as in JSON Schema.
5. **`regex-lite` as the engine.** It moves from a dev-dependency to a
   dependency; `Cargo.lock` does not change. It runs in time linear in the
   pattern and the value, which matters because both come from untrusted
   manifests and declarations (AGENTS.md §5). Its `\d`, `\w`, `\s` are
   ASCII, as in ECMAScript, and it refuses Unicode classes, look-around, and
   back-references at parse time, so a pattern either means the same in an
   editor's JSON Schema check or is refused. The `regex` crate was rejected:
   larger, and its Unicode `\d` and `\w` would accept values an editor
   rejects. MSRV: `regex-lite` 0.1.9 requires Rust 1.65.
6. **The schema.** `manifest.schema.json` declares `pattern` as a string and
   refuses it, like `allowed`, for types other than `string`.

## Consequences

- A template that inserts a string into JSON writes `"name": {{ name | tojson }}`
  without surrounding quotes; HTML uses `{{ name | e }}`. Templates without
  filters render as before.
- A template wanting another transformation still fails; the grammar grows
  only by an owner decision on OC-5.
- An author porting a JavaScript pattern with look-ahead or `\p{L}` must
  rewrite it; the error is the parser's message at the manifest path.
- A published unit that declares `pattern` cannot be read by an earlier
  Assemblr, which refuses the unknown member; that is the existing rule for
  any new manifest member.

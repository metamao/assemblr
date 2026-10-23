# Parameters and Dependency Options

Two different manifest concepts configure units:

- **`params`** is the unit's *own* public configuration contract: the inputs
  it accepts.
- **`options`** is a set of *strict requirements* the unit imposes on its
  dependencies' parameters.

## Parameters

### Purpose — Approved

Parameters are a unit's declared, public configuration inputs. They customize
**file contents**. They do not customize the generated project's file
structure.

### Types — Approved

The initial Alpha types are:

| Type | Notes |
|------|-------|
| string | |
| integer | |
| boolean | |
| enumerated string | A string restricted to declared allowed values |

Do not add parameter types or schema complexity without a demonstrated use
case.

### Declaration — Approved

Each parameter has:

- a **name**;
- a **type**;
- a **human-readable description**, which is required;
- an optional **default**;
- **allowed values**, optional, for constrained strings.

Values supplied for a parameter, and its default, are validated against its
type and allowed values.

The name `own` is **reserved** for the amending author's template context
(see
[amendments-and-composition.md](amendments-and-composition.md#template-contexts--approved)).
It is not a valid parameter name. Other names follow the [approved
rule](manifest.md#parameter-definitions-and-manifest-text--approved) (**OC-50**, resolved).

Requiredness follows from the default alone:

- A parameter **without a default is required**.
- A parameter **with a default is optional**. Its default applies when no
  value is supplied.
- There is no separate `required` property, and no "required with a default".
- A required parameter that receives no value is an explicit **validation
  error** naming the unit and the parameter. Assemblr never substitutes a
  value. Who supplies it is described in
  [Satisfying dependency parameters](#satisfying-dependency-parameters--approved)
  and [Consumer values](#consumer-values--approved). For a unit's own builds,
  see [Examples](#examples--approved).

The serialization of parameter declarations is described in
[manifest.md](manifest.md#serialization-of-dependencies-options-params-and-amendments--approved).

### Examples — Approved

Decided by the owner on #62 (decision log D-24). A parameter may declare an
**`example`**: a value of its type, and one of its allowed values, that the
unit is built, checked, and published with while the parameter is required
and has no default.

- An example is an authoring-time value. It is **not a default**: the
  parameter stays required, a consumer must supply it, and a composition never
  uses an example in place of a missing value. It does not change the unit's
  configuration in any composition and never silently becomes a consumer
  value. A diagnostic may show it as a suggestion.
- It must satisfy the parameter's type and allowed values. That does not
  prove that every value is valid: the values a composition resolves are
  validated as they are.
- `assemblr info` shows it. See the
  [authoring workflow](authoring-workflow.md#build-details--approved) for how
  a unit's own build uses it.

### What parameters may do — Approved

Parameters may affect **template interpolation inside files**, both unit files
and amendments.

### What parameters must not do — Approved

Parameters must not:

- change source file paths;
- dynamically rename generated files;
- conditionally create or omit files;
- dynamically alter the dependency graph;
- execute commands;
- act as a secrets-management mechanism.

### Rendering — Approved

Rendering replaces the references in a unit's **templates** with the unit's
resolved parameter values. The rules below are approved; the items listed
as open are not.

- **Templates are selected explicitly.** A `src/` file whose name ends in
  `.assemblr.tmpl` is a template: it is delivered at its path without that
  suffix, rendered. Every other file is delivered byte for byte, so files
  whose own syntax resembles the template syntax, such as Angular templates
  (`{{ value }}`) or GitHub Actions workflows (`${{ value }}`), need nothing
  (see [units.md](units.md#templates)).
- Rendering **must be deterministic**.
- Rendering is a **pure function of one file**: its source text and its
  context. No other file, and no state outside the context, takes part. A
  template cannot include or import anything, and it cannot reach the file
  system, the environment, the clock, or a shell.
- A unit file's **context** is the resolved parameter values of the unit that
  provides it. Amendment copies additionally see `own`, the amending unit's
  own values (see
  [amendments-and-composition.md](amendments-and-composition.md#template-contexts--approved)).
  `own` is reserved and is defined only there; a reference to it elsewhere is
  an undefined reference.
- **Syntax.** A reference is `{{ name }}`, naming a parameter of the unit,
  with ASCII whitespace allowed inside the braces. `{{ own.name }}` names
  the amending author's context. The escape `{{ "{{" }}` produces a literal
  `{{`. That is the whole grammar: there is no control flow, no filter, no
  expression, no include, and no other literal. Any other construct that
  opens with `{{`, `{%`, or `{#`, and any opener that is never closed, is a
  **rendering error**; nothing is passed through. A `}}` on its own is
  ordinary text. The grammar is a subset of Jinja's, so a richer engine
  could render the same templates identically later; none is planned for
  Alpha.
- A reference to an **undefined value is a rendering error**. It never
  renders as empty text.
- **Values render as text**: a string, including an enumerated string, as
  its characters, unchanged; an integer in decimal; a boolean as `true` or
  `false`. Rendering does **no** quoting and no format-specific escaping,
  for JSON, YAML, HTML, shell, or any other format. The template author
  writes the quotes, and a value containing quotes, newlines, or `{{` is
  inserted as it is. Inserted text is never rendered again.
- Outside its references a template is copied **byte for byte**: line
  endings, and a trailing newline or its absence, are preserved.
- A template must be **UTF-8 text**; one that is not is a rendering error.
  Files that are not templates are never inspected.
- Rendering errors are **collected**: every independently detectable problem
  in every template is reported with its file, line, column, and construct,
  ordered by delivered path and then position. In `unit build` they gate the
  build, and `build/` and the unit lockfile are not changed (see
  [authoring-workflow.md](authoring-workflow.md#build-details--approved)).
- Whether a file is rendered is decided by the suffix of the providing
  unit's file. An amendment copy of that file inherits the outcome.

Open:

- Whether constructs beyond references, such as conditional content or
  filters, are ever supported, and with which engine. If conditional content
  is ever added, it must not **silently discard intended changes**: research
  found that changes hidden by disabled template conditions can disappear
  silently (see
  [amendments-and-composition.md](amendments-and-composition.md#evidence-non-normative)).
  **OC-5.**

## Dependency options

### Purpose — Approved

`options` declares **strict configuration requirements** that a unit imposes on
the parameters of its **direct** dependencies.

Example (serialization as approved in
[manifest.md](manifest.md#serialization-of-dependencies-options-params-and-amendments--approved)):

```jsonc
{
  "dependencies": {
    "@metamao/node": "^1.0.0"
  },
  "options": {
    "@metamao/node": {
      "packageManager": "pnpm"
    }
  }
}
```

Here the unit requires its `@metamao/node` dependency to be configured with
`packageManager = pnpm`.

### Rules — Approved

- A unit's `options` may only target its **direct** dependencies: identities
  it declares in its own `dependencies`. Targeting a transitive dependency, or
  any other identity, is an error.
- Options may only configure parameters that the dependency **exposes** in its
  `params`. Configuring an undeclared parameter is an error.
- Option values are validated against the dependency parameter's type and
  allowed values.
- Two **identical** explicit requirements on the same dependency parameter are
  compatible.
- **Incompatible** explicit requirements produce an **options conflict**, which
  is reported.
- A parameter's **default is not a strict requirement**. A default never
  conflicts with an explicit requirement, and it is never treated as one.
- There is **no implicit last-writer-wins**. Neither declaration order nor
  dependency depth settles a conflict.
- Options conflicts and file merge conflicts are **distinct kinds of conflict**
  and are reported as such.

A consumer's values likewise target only its explicit selections (see
[Consumer values](#consumer-values--approved)).

### Satisfying dependency parameters — Approved

A published unit is **responsible for satisfying the required parameters of
its direct dependencies**. It does so through:

1. valid defaults of the dependency's parameters;
2. explicit values in its own `options`;
3. valid **mappings** from its own public parameters to dependency parameters.

Rules:

- A mapping may reference only parameters the unit declares in its `params`.
  Assemblr validates that each referenced parameter exists, and that its type
  and allowed values are compatible with the dependency parameter it
  configures.
- The required configuration of **transitive** dependencies is satisfied
  through the dependency chain: each unit satisfies its direct dependencies,
  which in turn satisfy theirs. A unit never configures a transitive
  dependency directly.
- `unit publish` validates these obligations across the unit's resolved
  dependency graph (see
  [authoring-workflow.md](authoring-workflow.md#publication--approved)).
- A consumer that selects a unit is never asked for the required parameters of
  that unit's transitive dependencies. If such a value must be configurable,
  the depending unit exposes it through its own `params`.
- Incompatible requirements on the same parameter of a shared dependency, for
  example from two units that both depend on it, remain an **options
  conflict**, with no implicit precedence (see [Rules](#rules--approved)).
- Mappings configure file contents only, like any parameter value. They never
  alter the dependency graph or which files exist (see
  [What parameters must not do](#what-parameters-must-not-do--approved)).

**Mappings — decided by the owner on #62 (decision log D-24).** In a unit's
`options`, an option value written `{ "from": "appName" }` is a mapping from
the unit's own parameter `appName`:

```jsonc
"params":  { "appName": { "type": "string", "description": "The application name.", "example": "my-app" } },
"options": { "@metamao/cloudflare-worker": { "appName": { "from": "appName" } } }
```

- A mapping is a **strict requirement** on the dependency's parameter whose
  value is the source parameter's **effective value**: the value the
  composition decided for it, a declared default included. It converts
  nothing, renames nothing by convention, and invents no value. Other
  option values are literals, as before. A consumer's `options` hold
  literals only; a mapping belongs to a unit's manifest.
- The source must be a parameter the unit declares. Assemblr rejects the
  known incompatibilities: the same type, or a string (plain or enumerated)
  mapped to a plain string, or an enumerated string whose allowed values the
  target also allows. A free string never maps to an enumerated string, and
  an integer, a boolean, and a string never map to one another. The value that
  results is validated against the dependency parameter's type and allowed
  values like any other requirement; an example proves nothing about it.
- Effective values are decided **from the dependents to the dependencies**: the
  consumer's values and literal options are requirements; each unit's value is
  the agreed requirement or its default; its mappings then produce
  requirements on the units below, through any number of levels. When several
  paths supply the same parameter, identical values agree, and different ones
  are an **options conflict** that names the requirements, the mapped ones
  with the source parameter they come from. No order settles a conflict.
- A required parameter whose value is missing is reported at the unit that
  must supply it, not again at the parameters it would have been mapped to.
- A unit that depends on units with required parameters must satisfy them
  (by a default, an option, or a mapping), so making a parameter required is a
  breaking change for the units that depend on it.
- The consumer-facing pattern: a **composition unit** depends on several
  units, exposes the configuration its consumers need once, and maps it to
  each dependency. A consumer of that unit sets the value once and need not
  know the graph.

## Consumer values — Approved

A consumer project supplies values to the parameters of the units it
explicitly selects, through `options` in its `assemblr.jsonc` (see
[consumer-projects.md](consumer-projects.md#parameters-and-non-interactive-operation--approved)).

A consumer value is an explicit requirement, and the [rules](#rules--approved)
for unit `options` apply to it unchanged:

- It may only configure a parameter the selected unit exposes, and it is
  validated against that parameter's type and allowed values.
- An identical requirement from a unit on the same parameter is compatible.
- An incompatible requirement, for example from another selected unit that
  depends on the same unit, is an **options conflict** and is reported. The
  consumer's value does not win by precedence, and neither does the unit's.
- A parameter's default never conflicts with a consumer value.

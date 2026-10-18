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
- a **required/default contract**, as applicable;
- **allowed values**, optional, for constrained strings.

Values supplied for a parameter are validated against its type and allowed
values.

Open:

- The exact required/default rules, for example whether a parameter without a
  default is implicitly required, and whether "required with a default" is
  meaningful. **OC-22.**
- The serialization of parameter declarations (see
  [manifest.md](manifest.md#vocabulary--approved)).

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

### Rendering — Approved direction, details Open

- Rendering **must be deterministic**.
- The expected direction is a mechanism compatible with Jinja-style
  templating. This is provisional. The engine and syntax are not yet chosen.
- If the selected template mechanism supports conditional content, it must not
  **silently discard intended changes**. Research found that changes hidden by
  disabled template conditions can disappear silently (see
  [amendments-and-composition.md](amendments-and-composition.md#evidence-non-normative)).

Open:

- The template engine, interpolation syntax, escaping, the handling of non-text
  files, and the detailed rendering rules. **OC-5.**

## Dependency options

### Purpose — Approved

`options` declares **strict configuration requirements** that a unit imposes on
a dependency's parameters.

Conceptual example (**not** canonical manifest serialization):

```text
dependencies:
  @metamao/node: ^1.0.0

options:
  @metamao/node:
    packageManager: pnpm
```

Here the unit requires its `@metamao/node` dependency to be configured with
`packageManager = pnpm`.

### Rules — Approved

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

Open:

- Whether `options` may target only direct dependencies or also transitive
  ones. **OC-21.**
- How option requirements interact with values supplied by a consumer project
  (reserved, **OC-1**) or by development build inputs (**OC-6**).

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

- Whether a unit's `options` may target only direct dependencies or also
  transitive ones. A consumer's values target only its explicit selections
  (see [Consumer values](#consumer-values--approved)). **OC-21.**
- How option requirements interact with development build inputs. **OC-6.**

### Satisfying dependency parameters — Approved

A published unit is **responsible for satisfying the required parameters of
its dependencies**. It does so through:

1. valid defaults of the dependency's parameters;
2. explicit values in its own `options`;
3. valid **mappings** from its own public parameters to dependency parameters.

Rules:

- A mapping may reference only parameters the unit declares in its `params`.
  Assemblr validates that each referenced parameter exists, and that its type
  and allowed values are compatible with the dependency parameter it
  configures.
- `unit publish` validates these obligations, including transitive
  compatibility (see
  [authoring-workflow.md](authoring-workflow.md#publication--approved)).
- A consumer that selects a unit is never asked for the required parameters of
  that unit's transitive dependencies. If such a value must be configurable,
  the depending unit exposes it through its own `params`.
- Mappings configure file contents only, like any parameter value. They never
  alter the dependency graph or which files exist (see
  [What parameters must not do](#what-parameters-must-not-do--approved)).

Open:

- The syntax of mappings, and how options conflicts are evaluated when an
  option value is mapped from a parameter. **OC-39.**
- The required/default rules that decide when a parameter needs a value.
  **OC-22.**

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

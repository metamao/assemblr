# Manifest

Every unit has a manifest named `manifest.jsonc` at the root of its unit
repository.

## Format — Approved

- The manifest format is **JSONC**: JSON with comments.
- The manifest is the source of the unit's identity, version, description,
  dependencies, parameters, dependency options, and amendment declarations.
- A consumer project declares its composition in `assemblr.jsonc`, which
  deliberately reuses the manifest's `dependencies` and `options` semantics but
  is not a unit manifest (see
  [consumer-projects.md](consumer-projects.md#project-configuration--approved)).

## Vocabulary — Approved

These fields make up the accepted conceptual vocabulary:

| Field | Purpose | Details |
|-------|---------|---------|
| `$schema` | Points editors at the manifest JSON Schema, where applicable. | [Editor support](#editor-support--approved) |
| `name` | The unit identity, `@owner/unit-name`. | [units.md](units.md#identity--approved) |
| `version` | The unit's exact SemVer version. | [units.md](units.md#versioning--approved) |
| `description` | A human-readable description. **Required.** | |
| `dependencies` | Owner-qualified identities with SemVer ranges. | [dependencies.md](dependencies.md) |
| `params` | The unit's public configuration contract. | [parameters-and-options.md](parameters-and-options.md#parameters) |
| `options` | Strict requirements imposed on dependency parameters. | [parameters-and-options.md](parameters-and-options.md#dependency-options) |
| amendment declarations | Declare amendments and their provenance. | [amendments-and-composition.md](amendments-and-composition.md) |

The **meaning** of each field above is approved. The amendment declaration
field name and syntax are **Open (OC-2)**. Do not treat earlier experimental
syntax, such as `extends` or a `merges` field, as canonical.

## Serialization of `dependencies`, `options`, and `params` — Approved

These three fields are **maps**:

| Field | Keys | Values |
|-------|------|--------|
| `dependencies` | Unit identities (`@owner/unit-name`) | A SemVer requirement string |
| `options` | Identities of the unit's **direct** dependencies | A map from parameter name to the value required for that parameter |
| `params` | Parameter names | The parameter's typed definition: its type, its description, an optional default, and optional allowed values |

- An option value or a default is written as the JSON value of the parameter's
  type: a string or an enumerated string as a JSON string, an integer as a JSON
  integer, and a boolean as a JSON boolean.
- Every key of `options` must also be a key of `dependencies` (see
  [parameters-and-options.md](parameters-and-options.md#rules--approved)).
- A parameter definition has no `required` property. Requiredness follows from
  the presence of a default (see
  [parameters-and-options.md](parameters-and-options.md#declaration--approved)).
- A consumer project's `assemblr.jsonc` reuses the `dependencies` and `options`
  serialization. It has no `params` (see
  [consumer-projects.md](consumer-projects.md#project-configuration--approved)).

Example. The structure is canonical. Identities, versions, and parameter names
are illustrative. The property names inside a parameter definition are not yet
fixed (**OC-45**), so that definition is elided:

```jsonc
{
  "name": "@metamao/cloudflare-angular-ssr",
  "version": "1.0.0",
  "description": "Angular SSR on Cloudflare Workers.",
  "dependencies": {
    "@metamao/cloudflare-worker": "^2.1.0"
  },
  "options": {
    "@metamao/cloudflare-worker": {
      "nodeCompat": true
    }
  },
  "params": {
    "appName": { /* type, description, optional default, optional allowed values */ }
  }
}
```

Open:

- The property names and type spellings inside a parameter definition.
  **OC-45.** A provisional default is described below.
- How an option value mapped from the unit's own parameter is written.
  **OC-39.**
- Amendment declarations (**OC-2**) and list identity hints (**OC-4**).

## Parameter definitions and manifest text — Recommended (provisional)

The owner has accepted these defaults for Alpha. The related entries stay open
until the owner settles them: **OC-45**, **OC-50**, and **OC-51**.

A parameter definition has these properties:

| Property | Required | Value |
|----------|----------|-------|
| `type` | Yes | `"string"`, `"integer"`, or `"boolean"` |
| `description` | Yes | A string that is not empty or only whitespace |
| `default` | No | A JSON value of the parameter's type. Its presence makes the parameter optional. |
| `allowed` | No | Only with `"type": "string"`, which makes the parameter an enumerated string. A non-empty array of strings with no duplicates. |

```jsonc
{
  "params": {
    "appName": { "type": "string", "description": "The application name." },
    "port": { "type": "integer", "description": "Local port.", "default": 8080 },
    "packageManager": {
      "type": "string",
      "description": "The package manager.",
      "allowed": ["npm", "pnpm", "bun"],
      "default": "pnpm"
    }
  }
}
```

Other provisional rules:

- **Parameter names** start with an ASCII letter and continue with ASCII
  letters, digits, or underscores, such as `appName` or `node_compat`. Names
  stay usable in any template syntax chosen later (OC-5), and the rule can be
  widened compatibly. **OC-50.**
- **JSONC.** Comments (`//` and `/* */`) and trailing commas after a value are
  accepted, as in common JSONC tooling. Other JSON5 extensions, such as
  single-quoted strings, unquoted keys, and hexadecimal numbers, are rejected.
  **OC-51.**
- **Duplicate keys** in any object are rejected rather than silently resolved.
  **OC-51.**
- **Integers** are 64-bit signed. A fraction, an exponent, or an integer
  outside that range is not an integer. **OC-51.**
- **Descriptions**, of the unit and of each parameter, must not be empty or
  only whitespace. **OC-51.**

## Validation — Approved

- Unknown fields are rejected.
- Invalid combinations of fields and values are rejected.
- A missing `description` is an error.
- Identity, version, parameter, option, and dependency declarations are
  validated as described in their respective documents.
- Validation errors are reported, never silently corrected.

Assemblr's own validation is **authoritative**. The JSON Schema provides editor
assistance but does not define validity on its own. Rules a schema cannot
express, such as cross-field and cross-unit consistency, are still enforced by
Assemblr.

## Editor support — Approved

- An initialized unit includes a **local** JSON Schema for the manifest that
  provides editor completion and diagnostics.
- The manifest references that schema through `$schema`.
- The schema works **offline**. Editing a unit must not require network access
  to fetch the schema.

Open:

- The schema's file name and location, whether it is tracked in Git, and when
  it is regenerated (for example, after an Assemblr upgrade). **OC-17.**

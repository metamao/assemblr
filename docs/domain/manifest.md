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

Established semantics vs pending syntax:

- The **meaning** of each field above is approved.
- The **serialization** of `dependencies`, `params`, and `options` (key shapes,
  nesting, value encodings) is not final. Examples in these documents are
  **Conceptual**. Finalizing it is part of the manifest schema contract, which
  includes **OC-2** (amendments), **OC-4** (list identity hints), **OC-22**
  (parameter requiredness and default rules), and **OC-39** (mapping option
  values from the unit's own parameters).
- The **amendment declaration field name and syntax** are **Open (OC-2)**. Do
  not treat earlier experimental syntax, such as `extends` or a `merges`
  field, as canonical.

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

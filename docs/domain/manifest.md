# Manifest

Every unit has a manifest named `manifest.jsonc` at the root of its unit
repository.

## Format — Approved

- The manifest format is **JSONC**: JSON with comments.
- The manifest is the source of the unit's identity, version, description,
  dependencies, placeholder declarations, and amendment declarations.
- A consumer project declares its composition in `assemblr.jsonc`, which
  deliberately reuses the manifest's `dependencies` semantics but is not a
  unit manifest (see
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
| `placeholders` | The placeholders the unit's files hold for the consumer to replace, and the tokens that only look like placeholders. | [placeholders.md](placeholders.md#declaration--approved) |
| `amendments` | Declares the unit's amendments and their provenance. | [amendments-and-composition.md](amendments-and-composition.md#layout-and-declaration) |

The **meaning** of each field above is approved. An amendment declaration
names the amended path, the unit below that provides it, and the exact base
version (see
[amendments-and-composition.md](amendments-and-composition.md#layout-and-declaration)).
Earlier experimental syntax, such as `extends` or a `merges` field, is not
canonical. `params` and `options` no longer exist; a manifest that holds
either is rejected with an error that explains the replacement (see
[placeholders.md](placeholders.md#removed)).

## Serialization of `dependencies`, `placeholders`, and `amendments` — Approved

| Field | Form |
|-------|------|
| `dependencies` | A map from unit identities (`@owner/unit-name`) to SemVer requirement strings |
| `placeholders` | An object with two optional members: `replace`, a map from placeholders to non-empty descriptions, and `ignore`, an array of distinct placeholder-form tokens |
| `amendments` | A map from amended paths, the file's path in the provider's `src/`, which is the copy's path under `amendments/`, to an object with `unit`, the identity of the **direct** dependency that provides the file, and `base`, the exact version the copy was taken from, as a string |

- Every `unit` of an amendment must be a key of `dependencies`, and every
  amended path follows the `src/` path rules (see
  [units.md](units.md#src-unit-files)). `base` is an exact version, not a
  requirement, and it must equal that dependency's locked version (see
  [amendments-and-composition.md](amendments-and-composition.md#layout-and-declaration)).
- Every key of `replace` and entry of `ignore` has the placeholder form (see
  [placeholders.md](placeholders.md#form--approved)).
- A consumer project's `assemblr.jsonc` reuses the `dependencies`
  serialization (see
  [consumer-projects.md](consumer-projects.md#project-configuration--approved)).

Example. The structure is canonical. Identities, versions, and placeholders
are illustrative:

```jsonc
{
  "name": "@metamao/cloudflare-angular-ssr",
  "version": "1.0.0",
  "description": "Angular SSR on Cloudflare Workers.",
  "dependencies": {
    "@metamao/cloudflare-worker": "^2.1.0"
  },
  "placeholders": {
    "replace": {
      "__APP_TITLE__": "The application's visible name."
    }
  },
  "amendments": {
    "wrangler.toml": { "unit": "@metamao/cloudflare-worker", "base": "2.1.0" }
  }
}
```

Open:

- List identity hints (**OC-4**).

## Manifest text — Approved

Accepted by the owner as defaults for Alpha and approved in the
[owner review of 2026-10-23](https://github.com/metamao/assemblr/issues/34#issuecomment-6358234799): **OC-51** is resolved.

- **JSONC.** Comments (`//` and `/* */`) and trailing commas after a value are
  accepted, as in common JSONC tooling. Other JSON5 extensions, such as
  single-quoted strings, unquoted keys, and hexadecimal numbers, are rejected.
  **OC-51.**
- **Duplicate keys** in any object are rejected rather than silently resolved.
  **OC-51.**
- **Descriptions**, of the unit and of each placeholder, must not be empty or
  only whitespace. **OC-51.**

## Validation — Approved

- Unknown fields are rejected.
- Invalid combinations of fields and values are rejected.
- A missing `description` is an error.
- Identity, version, dependency, placeholder, and amendment declarations are
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

The schema is `manifest.schema.json` next to the manifest, tracked, written
by `unit init` and refreshed by running it again; no command compares it
(**OC-17**, resolved; D-2, see
  [authoring-workflow.md](authoring-workflow.md#initialization-details--provisional-d-5-d-33)).

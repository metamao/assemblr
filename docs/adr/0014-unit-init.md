# 0014: `unit init`: repository discovery and initialization, the scaffold's writes, and the embedded manifest schema

- **Status:** Accepted
- **Date:** 2026-10-21

## Context

The second PR of the authoring-completeness increment (#32) adds `unit
init`, which closes the authoring loop at its start. The product rules are
in
[`authoring-workflow.md`](../domain/authoring-workflow.md#initialization-details--provisional-d-2-d-5)
and the decisions behind them in the autonomous decision log (D-2, D-5).
This ADR records how they are implemented: how the directory's place in Git
is found and a repository created, given that Assemblr otherwise runs Git in
a scrubbed environment (ADR 0006) and that `unit publish` already runs one
write with the author's configuration (ADR 0013); how the scaffold is
written so that the command is safe to run again; and how the manifest's
JSON Schema is produced and tested.

## Decision

1. **Discovery runs Git as the user would, read-only.** `discover_repository`
   runs `git rev-parse --is-bare-repository` and `--show-toplevel` in the
   directory, or in its nearest existing ancestor when it does not exist
   yet, with ADR 0006's environment and arguments but **without** `GIT_DIR`,
   so Git searches the parents as it does for the user. "Not a git
   repository" on stderr means none; any other failure is an error. Since
   that reads one of Git's messages, the shared read environment now also
   sets `LC_ALL=C` and `LANGUAGE=C`, extending ADR 0006, so no read depends
   on the author's locale; CI installs no other locale, so this is not
   exercised there. The top
   level is compared with the directory through `fs::canonicalize` on both,
   which absorbs Git's forward slashes on Windows and symbolic links in the
   path. The outcomes are: no repository (initialize), the directory is the
   top level (adopt, unless `.git` is a file, which is a linked worktree the
   build cannot read), inside a repository whose top level holds
   `manifest.jsonc` (refuse), inside any other repository (initialize a
   nested one), bare (refuse).
2. **Initialization is the author's write.** `init_repository` runs `git
   init --quiet` in the directory the way ADR 0013 creates the tag: the
   author's global and system configuration apply, so `init.defaultBranch`
   and `init.templateDir` are theirs; the location variables are removed;
   network and prompts stay disabled. `git init` inside another working tree
   creates a nested repository, which is what the decision asks for.
3. **Decide, then write in a recoverable order.** Every input is read
   first: the existing manifest, if any, through the manifest reader, and
   the schema and `.gitignore` as bytes, each bounded at 1 MiB and refused
   when not a regular file. Then the writes: the directory (`create_dir_all`),
   the repository, `manifest.jsonc`, `manifest.schema.json`, `.gitignore`.
   New files are created with `create_new`; a replaced file is written to a
   reserved name beside it and renamed. A failure reports what was written,
   and the same command completes the rest, because every step is decided
   from the current state. The manifest text is produced by formatting and
   read back through `read_manifest` under `debug_assert!`, so the scaffold
   can never write a manifest Assemblr rejects.
4. **The schema is a fixed document.** `src/adapters/manifest.schema.json`
   is embedded with `include_str!` and written byte for byte. It is JSON
   Schema draft 7, which every mainstream editor's JSON language service
   supports, including for JSONC files. It expresses the approved vocabulary
   and the provisional parameter-definition rules with `additionalProperties:
   false`, `propertyNames` patterns for identities, parameter names, and
   amendment paths, the SemVer pattern, a non-blank pattern for
   descriptions, and `if`/`then`/`else` for the default's type and for
   `allowed` being a string-only property. The reserved parameter name `own`
   is excluded with `not: { const }`. Cross-field rules (options naming
   declared dependencies, bases equal to locked versions, defaults among
   allowed values) are left to Assemblr. Generating the schema from the
   reader's tables was considered and rejected: the document is small, the
   tables do not carry descriptions, and a generator would be more code
   than the document it generates.
5. **Testing the schema without a validator.** A full JSON Schema validator
   (`jsonschema`) would add about 75 crates to the test build for a document
   that is editor assistance only. Instead, `regex-lite` (no dependencies)
   compiles each pattern in tests, and the tests assert that the patterns
   agree with `UnitIdentity::parse`, `Version::parse`, and
   `ParameterName::parse` on a corpus of accepted and rejected inputs, and
   that the keyword structure matches the vocabulary (`MANIFEST_FIELDS`),
   the required fields, and the parameter rules. The document was also
   validated once by hand with Ajv in strict mode against a full manifest
   and twelve invalid ones; that check is not part of the test suite.
   Integration tests run the binary: a fresh directory, an adopted
   repository, a nested one, the refusals, the second run, and the loop
   from `unit init` through `unit publish` to a dependent's build against a
   second registry.

## Consequences

- `unit init` is the second command that runs Git with the author's
  configuration (after the tag of ADR 0013), and the first that discovers a
  repository rather than being given one.
- A manifest's `$schema` refers to a tracked file beside it, so a clone is
  editable offline, and the schema can lag the installed Assemblr until
  `unit init` is run again. Nothing but editor assistance depends on it.
- The schema's patterns and the domain parsers can drift only if the tests
  are not updated with the parser, since the tests compare them on a
  shared corpus; widening the parameter-name rule (OC-50) is a two-place
  change.
- The test build gains one dependency-free crate.

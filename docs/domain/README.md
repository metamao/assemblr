# Assemblr Domain Contracts

This directory defines what Assemblr means and promises. It is the canonical
product specification for the production implementation.

Engineering rules for how Assemblr is built live in [`AGENTS.md`](../../AGENTS.md).
Architectural decisions live in [`docs/adr/`](../adr/).

## Product in one paragraph

Assemblr is a general-purpose, technology-agnostic Rust CLI that composes
software projects from versioned, reusable **units**. A unit is an
author-owned, opinionated implementation of a software capability or
integration, such as `@metamao/cloudflare-worker`. Another author may publish
a different implementation of the same technology, such as
`@alice/cloudflare-angular-ssr`; no implementation is inherently official or
privileged. Assemblr is a composition tool. It does not prescribe how
applications are written. Humans and coding agents are both intended users.
Agents matter especially, because they can inspect a unit's metadata and
parameter contracts, incorporate it, and move on to application-specific code.

## Sources of authority

When sources disagree, the higher entry wins:

1. **Approved product contracts in `docs/domain/`** (this directory).
2. **[`AGENTS.md`](../../AGENTS.md)** for engineering and architectural instructions.
3. **Implementation and tests**, as evidence of actual behavior.
4. **Historical research and experiments**, as supporting evidence only.

Earlier prototypes and research experiments informed these contracts but never
define them. Their specifications, syntax, and tests are not binding unless a
contract here adopts them explicitly. Research findings that support approved
decisions are summarized in this directory, so the contracts stand on their
own.

## Status labels

Every contract in this directory carries one of these labels:

| Label | Meaning |
|-------|---------|
| **Approved** | Decided by the owner. Implementations must conform. |
| **Conceptual** | Illustrates approved intent. The syntax, serialization, or command grammar shown is *not* canonical. |
| **Approved direction** | The intent is approved. The details are listed explicitly as Open items in the same section. |
| **Recommended (provisional)** | A default mechanism or policy the owner has accepted for Alpha. It is not a verified guarantee and may be revised when technical evidence shows a better approach. A revision that affects approved user-visible semantics needs owner review. |
| **Open (OC-n)** | A listed point that is not decided. Must not be implemented by assumption. Tracked in [open-contracts.md](open-contracts.md), which separates genuinely unresolved contracts from open details of approved semantics. |

Statements in **Approved** sections are **normative**: "must", "must not",
"never", and plain statements of behavior ("`build/` is owned by Assemblr")
are requirements. Example manifest serializations and command lines are
**Conceptual** unless they are explicitly marked canonical. Conventions stated
as rules, such as the `@owner/unit-name` identity form, the `v<version>` tag
form, and the `build/` directory, are approved.

If a behavior is not described here, it is unspecified. Do not infer it from
examples, from earlier prototypes, or from what "seems natural". Raise
it as an open contract instead.

## Scope

These documents cover the full lifecycle of **authoring, developing,
maintaining, validating, and publishing units**, plus the local registry
contracts needed to support that lifecycle: the CLI-managed registry,
registration of unit repositories and catalogs, synchronization, and
discovery.

### Reserved: the consumer-project lifecycle

How consumer projects are created, how they persist compositions, how units are
added, removed, or updated in them, how consumer-specific configuration is
handled, and the consumer CLI UX are **intentionally not specified**. That
domain is reserved for a future owner-led design discussion. These documents
mention consumers only where unit authoring needs to.

Two points are already approved:

- `assemblr init` is explicit, and it is required before a project consumes
  units.
- `init` prepares only minimal Assemblr project configuration, conceptually an
  `assemblr.jsonc` file. It does not generate application code, resolve or
  install units, or modify existing application files.

Lock and historical state may be created lazily, when needed. The project
manifest schema, the lockfile layout, and the rest of the consumer lifecycle
are not specified.

## Documents

| Document | Covers |
|----------|--------|
| [units.md](units.md) | Unit identity, versioning, repository convention, unit contents |
| [manifest.md](manifest.md) | `manifest.jsonc` format, vocabulary, schema, and validation |
| [parameters-and-options.md](parameters-and-options.md) | `params` (a unit's public configuration) and `options` (requirements on dependencies) |
| [dependencies.md](dependencies.md) | Dependency declarations and resolution |
| [amendments-and-composition.md](amendments-and-composition.md) | Amendments, template contexts, merge semantics, and supporting evidence |
| [registries-and-discovery.md](registries-and-discovery.md) | The CLI-managed local registry, Git-based units, catalogs, registration, synchronization, immutability, removal, `search` and `info` |
| [authoring-workflow.md](authoring-workflow.md) | `unit init`, `amend`, `check`, `build`, `update`, `publish`, and author responsibilities |
| [open-contracts.md](open-contracts.md) | Register of unresolved contracts |

## Glossary

| Term | Meaning |
|------|---------|
| **Unit** | A versioned, author-owned, opinionated implementation of a software capability or integration. |
| **Owner** | The individual or organization namespace in a unit identity (`@owner`). |
| **Identity** | The pair `@owner/unit-name`. It uniquely identifies a logical unit and implies nothing about dependencies or repository location. |
| **Manifest** | `manifest.jsonc`, the declaration of a unit's identity, version, description, dependencies, params, options, and amendments. |
| **Unit repository** | The Git repository that holds exactly one unit's authoring sources and history. Every unit has one. |
| **Working tree** | The author's editable checkout of a unit repository. Mutable, and never a published version. |
| **Published version** | A unit version established by `unit publish` as a `v<version>` Git tag whose manifest declares exactly that version. Immutable. |
| **Unit file** | A file in `src/`, introduced by the unit. |
| **Amendment** | An editable full-file copy of a file provided by a dependency, modified by the amending unit. Replaces the earlier term *fragment*. |
| **Base** | The exact historical version of a dependency file that an amendment was copied from. |
| **Provenance** | The recorded origin of an amendment: the dependency, its exact version, and the file it was copied from. |
| **Parameter** (`params`) | A declared, public configuration input of a unit that customizes file contents. |
| **Option** (`options`) | A strict requirement that a unit imposes on a parameter of one of its dependencies. |
| **Composition** | A unit together with its resolved dependency graph and their combined contributions. |
| **Contribution** | What a unit adds beyond its dependencies: files, amendments, or meaningful configuration of dependencies. |
| **Local registry** | The CLI-managed subsystem, global to the user's Assemblr installation, that records registered sources, their published versions, and discoverable metadata. |
| **Registered source** | A unit repository or a catalog that has been explicitly registered and validated. |
| **Catalog** | A Git repository, managed through the CLI, that declares a set of unit repositories by identity and Git source without containing them. |
| **Catalog manifest** | The catalog's declarative manifest, provisionally `registry.jsonc`. |
| **Registration** | Validating a source and adding it to the local registry. It never publishes. |
| **Synchronization** | Explicitly refreshing the registry's knowledge of registered sources and their published versions. |
| **Registry index** | The registry's discoverable, refreshable metadata about registered sources and versions. |
| **Snapshot** | A fixed, identifiable copy of a unit's content at a resolved version, with its Git provenance. |
| **Lock state** | The record of the exact resolved versions (and their sources and content identity) used for a composition. |
| **Build directory** | `build/`, the Assemblr-owned, disposable directory where `unit build` writes a composed result. |
| **Options conflict** | Two incompatible explicit option requirements on the same dependency parameter. |
| **Merge conflict** | Incompatible changes from several contributors to the same file content. |

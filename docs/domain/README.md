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
Agents matter especially, because they can inspect a unit's metadata and the
placeholders it declares, incorporate it, and move on to application-specific
code.

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
| **Recommended (provisional)** | A default mechanism or policy the owner has accepted for Alpha. Alpha implementations may follow it; doing so is not implementing by assumption. It is not a verified guarantee or a final decision: the contract that cites it stays open until the owner settles it, and the default may be revised when technical evidence shows a better approach. A revision that affects approved user-visible semantics needs owner review. |
| **Provisional (D-n)** | Implemented behavior an agent decided on an open point under [AGENTS.md §1.1](../../AGENTS.md#11-decisions-autonomy-by-default-escalation-by-exception), recorded as entry D-n of the [autonomous decision log](https://github.com/metamao/assemblr/issues/34). Implementations follow it, but it is not approved: the open contract it answers stays open, and the owner may revise it. Merging the pull request that implements it is not that approval. Only the owner's explicit decision, such as an answer on an issue, makes it **Approved**, and the label changes with that decision. In a section labeled Provisional, a statement that cites an owner decision is approved. |
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

Only the owner's acceptance makes a recommendation binding or usable as a
default. A recommendation the owner has not accepted, for example one made in
research, an experiment, or a review, is not a requirement, even when it
appears in an open contract's context. An agent may implement a
**provisional decision** on an open point within its delegated authority
([AGENTS.md §1.1](../../AGENTS.md#11-decisions-autonomy-by-default-escalation-by-exception)),
recorded in the autonomous decision log. That is neither approval nor
resolution: the behavior stays provisional, the open contract stays open, and
only the owner's decision makes it approved. Such a statement is labeled
**Provisional (D-n)**, inline or in a section heading, so it is never read as
approved text.

## Scope

These documents cover:

- the full lifecycle of **authoring, developing, maintaining, validating, and
  publishing units**;
- the **local registry**: the CLI-managed registry, registration of unit
  repositories, synchronization, and discovery;
- the **consumer-project lifecycle**: initializing a project, declaring its
  composition in `assemblr.jsonc`, adding and removing units, composing, file
  contributions, reconciliation, conflict resolution, and lock state.

Details that are still open are listed in
[open-contracts.md](open-contracts.md).

## Documents

| Document | Covers |
|----------|--------|
| [units.md](units.md) | Unit identity, versioning, repository convention, unit contents |
| [manifest.md](manifest.md) | `manifest.jsonc` format, vocabulary, schema, and validation |
| [placeholders.md](placeholders.md) | Placeholders: what a unit declares, the author's checks, and the consumer's report |
| [dependencies.md](dependencies.md) | Dependency declarations and resolution |
| [amendments-and-composition.md](amendments-and-composition.md) | Amendments, merge semantics, and supporting evidence |
| [registries-and-discovery.md](registries-and-discovery.md) | The CLI-managed local registry, Git-based units, registration, synchronization, immutability, removal, `search` and `info` |
| [authoring-workflow.md](authoring-workflow.md) | `unit init`, `amend`, `check`, `build`, `update`, `publish`, and author responsibilities |
| [consumer-projects.md](consumer-projects.md) | `init`, `assemblr.jsonc`, `add`, `remove`, `compose`, explicit and transitive dependencies, file contributions, reconciliation, conflict resolution, and the lockfile |
| [open-contracts.md](open-contracts.md) | Register of unresolved contracts |

## Glossary

| Term | Meaning |
|------|---------|
| **Unit** | A versioned, author-owned, opinionated implementation of a software capability or integration. |
| **Owner** | The individual or organization namespace in a unit identity (`@owner`). |
| **Identity** | The pair `@owner/unit-name`. It uniquely identifies a logical unit and implies nothing about dependencies or repository location. |
| **Manifest** | `manifest.jsonc`, the declaration of a unit's identity, version, description, dependencies, placeholders, and amendments. |
| **Unit repository** | The Git repository that holds exactly one unit's authoring sources and history. Every unit has one. |
| **Working tree** | The author's editable checkout of a unit repository. Mutable, and never a published version. |
| **Published version** | A unit version established by `unit publish` as a `v<version>` Git tag whose manifest declares exactly that version. Immutable. |
| **Unit file** | A file in `src/`, introduced by the unit. |
| **Amendment** | An editable full-file copy of a file provided by a dependency, modified by the amending unit. Replaces the earlier term *fragment*. |
| **Base** | The exact historical version of a dependency file that an amendment was copied from. |
| **Provenance** | The recorded origin of an amendment: the dependency, its exact version, and the file it was copied from. |
| **Placeholder** | A literal token such as `__PROJECT_NAME__` that a unit's files hold for a value each project sets, declared with a description in the manifest. Assemblr reports it and never substitutes it; the consumer replaces it. |
| **Composition** | A unit, or a consumer project's explicit selections, together with the resolved dependency graph and their combined contributions. |
| **Contribution** | What a unit adds beyond its dependencies: files or amendments. |
| **Local registry** | The CLI-managed subsystem, global to the user's Assemblr installation, that records registered sources, their published versions, and discoverable metadata. |
| **Registered source** | A unit repository that has been explicitly registered and validated. |
| **Source** | The Git repository reference from which a unit is obtained. Different references are never assumed to denote the same repository. |
| **Registration** | Validating an untrusted source and adding it to the local registry, atomically. It never publishes. |
| **Synchronization** | Explicitly refreshing registered units, validating the complete result, and replacing the index only if it is valid. |
| **Registry index** | The registry's validated, discoverable metadata about registered sources, units, and published versions. Read offline. |
| **Integrity conflict** | Different content for the same unit identity and published version. |
| **Snapshot** | A fixed, identifiable copy of a unit's content at a resolved version, with its Git provenance. |
| **Lock state** | The record of the exact resolved versions (and their sources and content identity) used for a composition. A unit repository keeps it in its CLI-managed **unit lockfile**, and a consumer project in its own CLI-managed lockfile. The two are distinct. |
| **Build directory** | `build/`, the Assemblr-owned, disposable directory where `unit build` writes a composed result. |
| **Merge conflict** | Incompatible changes from several contributors to the same file content. |
| **Path collision** | Two units in one composition that provide a file at the same path through their `src/` directories. An error. |
| **Consumer project** | A software project composed from units, initialized with `assemblr init`. The final consumer of units. |
| **Project configuration** | `assemblr.jsonc`, the editable declaration of a consumer project's desired composition: its explicit `dependencies`. |
| **Explicit selection** | A unit declared in a consumer project's `dependencies`. |
| **Transitive dependency** | A unit present in a composition only because another unit requires it. |
| **Reconciliation base** | The previously materialized composition of a consumer project, used with the **current** files and the **incoming** composition for three-way reconciliation. Distinct from an amendment's base. |
| **Pending-resolution state** | The explicit state of a consumer project whose composition stopped on unresolved conflicts, until it is continued or aborted. |

# Open Contracts

This register lists every point in the domain documents that is not fully
decided. It has three parts:

- **[Unresolved contracts](#unresolved-contracts)**: product questions with no
  decision yet. Each needs an owner decision before it is implemented.
- **[Approved semantics with open details](#approved-semantics-with-open-details)**:
  the behavior is approved and normative, and implementations must conform to
  it. What remains open is the listed detail, such as syntax, formats,
  algorithms, or edge cases. A detail that changes user-visible behavior, syntax,
  or file formats still needs an owner decision. A purely internal
  implementation choice is an engineering decision, recorded in
  [`docs/adr/`](../adr/README.md) when significant.
- **[Resolved contracts](#resolved-contracts)**: entries settled or superseded
  by a decision, kept so their numbers stay traceable.

Implementations must not resolve any entry by assumption. When a decision
settles an entry, update the relevant domain document in the same change, then
mark the entry **Resolved** here with a link to the decision. Numbers are
stable and never reused.

**Origin** tells where an entry came from:

- **Owner**: explicitly left open by the owner when these contracts were
  approved.
- **Foundation**: a gap found while writing these documents. The owner did not
  list it explicitly, so it needs owner confirmation that it is a real
  question.
- **Registry**: a gap found while incorporating the approved registry model
  (see [ADR 0003](../adr/0003-managed-local-registry.md)). It needs owner
  confirmation that it is a real question.

Some entries cite a **Recommended (provisional)** default. A recommended default
is not a decision: the entry stays open until the owner settles it.

## Unresolved contracts

| ID | Question | Origin | Context |
|----|----------|--------|---------|
| OC-1 | **Consumer-project lifecycle.** Creating consumer projects; persisting compositions; adding, removing, and updating units; consumer-specific configuration; consumer CLI UX. **Reserved** for an owner-led design discussion. Already approved: `assemblr init` is explicit and required, and prepares only minimal project configuration. | Owner | [README](README.md#reserved-the-consumer-project-lifecycle) |
| OC-12 | Ordering policy for insertions at the same anchor. Approved so far: when the order may be significant, a conflict is preferred to an arbitrary order. The exact policy needs further validation. | Owner | [ordering](amendments-and-composition.md#ordering) |
| OC-13 | Whether Alpha provides a remote push operation, and its command or flag. Local publication and pushing are distinct operations. Recommended (provisional): `unit publish` never pushes as a side effect, and pushing is explicit. | Owner | [publication](authoring-workflow.md#publication--approved) |
| OC-14 | Owner syntax (character set) and boundary rules for both identity segments: leading, trailing, or consecutive hyphens, and length limits. | Foundation | [identity](units.md#identity--approved) |
| OC-15 | SemVer range dialect, and handling of pre-release and build metadata. Recommended (provisional) starting point: Cargo-like SemVer requirement semantics. | Foundation | [versioning](units.md#versioning--approved) |
| OC-16 | Policy for choosing among several versions that satisfy every range. It must be deterministic. Approved: existing lock state takes precedence over fresh resolution. Recommended (provisional): preserve satisfying locks, and otherwise prefer the highest compatible stable published version across the full transitive constraint set. | Foundation | [resolution](dependencies.md#resolution--approved), [version selection](dependencies.md#version-selection--recommended-provisional) |
| OC-18 | Behavior when a `src/` file has the same path as a file a dependency provides. | Foundation | [unit contents](units.md#src-unit-files) |
| OC-19 | Whether an amendment can delete an entire dependency file, or only change content. | Foundation | [amendments](amendments-and-composition.md#amendments--approved) |
| OC-21 | Whether `options` may target only direct dependencies or also transitive ones. | Foundation | [options](parameters-and-options.md#rules--approved) |
| OC-22 | Exact parameter required/default rules, for example whether a parameter without a default is implicitly required. | Foundation | [parameters](parameters-and-options.md#declaration--approved) |
| OC-26 | Publication edge cases: untracked files and the clean-state requirement, an existing tag that already points at the validated content, annotated vs lightweight tags (recommended (provisional): consider annotated tags), and whether a new version must be greater than earlier published versions. | Foundation | [publication](authoring-workflow.md#publication--approved) |
| OC-30 | **Development-time dependencies on unpublished units.** Whether a unit under development may resolve a dependency to another registered unit's unpublished working tree, as the retired Local Directory registry allowed, and if so how that source is selected and how lock state and snapshots treat it. Approved constraints: registration never publishes, and a published version is never substituted by a working tree. | Registry | [superseded model](registries-and-discovery.md#superseded-model-non-normative) |
| OC-31 | **Freshness and fetching outside synchronization.** Whether newly published tags of a registered local repository (for example, created by `unit publish`) become visible without `registry sync`, whether `unit publish` updates the registry index, and whether resolution in `check`, `build`, or `update` may fetch a missing source or version or must fail and ask for synchronization. Approved constraint: discovery never fetches implicitly. | Registry | [synchronization](registries-and-discovery.md#synchronization--approved) |

## Approved semantics with open details

| ID | Approved (normative) | Open detail | Origin | Context |
|----|----------------------|-------------|--------|---------|
| OC-2 | Amendments are declared in the manifest, with provenance and exact base. No earlier experimental syntax, such as `extends` or a `merges` field, is canonical. | Manifest schema and syntax for amendment declarations. | Owner | [amendments](amendments-and-composition.md#amendments--approved) |
| OC-3 | Amendments are full-file copies under `amendments/` that preserve provenance and exact base. | On-disk layout and naming of copies, and how provenance metadata is stored. | Owner | [amendments](amendments-and-composition.md#amendments--approved) |
| OC-4 | List identity rules may be needed where identity cannot be inferred safely. Identity is never guessed. | Manifest schema for list identity hints. | Owner | [merge semantics](amendments-and-composition.md#structured-and-unknown-formats) |
| OC-5 | Parameters affect file contents only. Rendering is deterministic. `own.*` is reserved for the amending author's context. Originals and amendments are rendered against compatible base contexts. Conditional content must not silently discard changes. | Template engine and syntax, escaping, non-text files, the exact rendering protocol, and the contents of `own.*`. | Owner | [rendering](parameters-and-options.md#rendering--approved-direction-details-open), [template contexts](amendments-and-composition.md#template-contexts--approved) |
| OC-6 | Required parameters without defaults must be supplied explicitly for development builds. | The mechanism for supplying them. | Owner | [build](authoring-workflow.md#build--approved) |
| OC-7 | The authoring operations (`init`, `amend`, `check`, `build`, `update`, `publish`), the registry operations (registering a source, creating a catalog and adding units to it, listing, synchronizing, unregistering), and discovery (`search`, `info`, with a machine-readable form) and their intent. `unit init` establishes the unit's Git repository. | Exact grammar and semantics of every command, flag, and argument, including how `unit init` establishes the Git repository (new or existing repository, a target inside another repository, an initial commit). | Owner | [commands](authoring-workflow.md#commands--conceptual), [initialization](authoring-workflow.md#initialization--approved), [registry](registries-and-discovery.md) |
| OC-8 | Versions and content are identifiable and verifiable. Working trees, published versions, the registry index, snapshots, and lock state are distinct. Snapshots preserve the provenance amendments need. Snapshots and lock state support deterministic reuse and reconciliation. Existing lock state takes precedence over fresh resolution. Historical content is never silently replaced. Recommended (provisional): content-addressed snapshots, and no automatic garbage collection of historical snapshots in Alpha. | Packaging, cache, snapshot, and lock-state formats and locations, including how content identity is computed and how long historical snapshots are retained. | Owner | [integrity](registries-and-discovery.md#integrity-and-snapshots--approved) |
| OC-9 | `unit check` verifies that every amendment's historical base is available. | Detailed behavior and diagnostics when historical Git objects (amendment bases, published versions) or registered sources are unavailable, for example a repository unreachable from another machine. | Owner | [amendments](amendments-and-composition.md#amendments--approved) |
| OC-10 | Copies are the authoring interface and operations the internal engine. Merge semantics as specified: deterministic, no silent loss, conflicts reported, conservative handling of unknown formats. | Production merge algorithms and format adapter selection. | Owner | [merge semantics](amendments-and-composition.md#merge-semantics--approved) |
| OC-11 | A unit with no effective contribution (files, amendments, or meaningful dependency configuration) is invalid for publication. This is not a "`src/` must contain files" rule. | The precise effective-contribution (no-op) algorithm. | Owner | [check](authoring-workflow.md#check--approved) |
| OC-17 | `unit init` provides a local, offline manifest JSON Schema referenced through `$schema`. Assemblr's own validation is authoritative. | The schema's file name and location, whether it is tracked in Git, and when it is regenerated. | Foundation | [editor support](manifest.md#editor-support--approved) |
| OC-20 | `unit amend` searches recursively by filename. Zero matches fail. Several matches are ambiguous and fail, listing the paths. An explicit relative path disambiguates. | Which file tree the search covers: the dependency's own files, or its composed output including files from its own dependencies. | Foundation | [amend](authoring-workflow.md#amend--approved) |
| OC-23 | *Revised by the registry model.* The CLI manages one global local registry. Unit repositories and catalogs are registered explicitly and validated. Re-registering the same source is idempotent. A different repository claiming a registered identity is a collision error. Nothing is registered implicitly, and there is no silent precedence. Users do not edit registry configuration by hand. | How a source conflict is resolved explicitly, including whether the earlier mechanisms (explicit source selection taking priority, owner-to-source mappings) remain part of the model, which needs owner confirmation. How two references are recognized as the same source (for example, a local path and a remote URL). | Foundation | [registration](registries-and-discovery.md#registration--approved), [source conflicts](registries-and-discovery.md#source-conflicts--approved) |
| OC-25 | Every unit is a Git repository. Tags are `v<version>` and must match the manifest version at the tagged commit exactly. A version tag whose manifest declares a different version is invalid, and only that version fails validation. A moved tag is detected and never trusted. | How invalid and moved tags are reported, what happens to a version after a moved tag is detected, and how tags that are not of the form `v<version>` are treated. | Foundation | [Git-based units](registries-and-discovery.md#units-are-git-repositories--approved), [integrity](registries-and-discovery.md#integrity-and-snapshots--approved) |
| OC-27 | Updates are explicit. Changing a resolved version and changing a declared range are separate concerns. Ranges are never widened automatically. | The command surface for each (see OC-7). Version choice within a range follows OC-16. | Foundation | [update](authoring-workflow.md#update--approved) |
| OC-28 | `unit build` performs the applicable native validations before generating output. | Exactly which `check` validations apply to `build`, for example whether the effective-contribution check also gates build. | Foundation | [build](authoring-workflow.md#build--approved) |
| OC-29 | Discovery is metadata-based, not semantic or AI-driven. `info` content is specified. Human and machine output derive from the same data. | Exact `search` matching rules and deterministic result ordering. | Foundation | [discovery](registries-and-discovery.md#discovery-and-information--approved) |
| OC-32 | A catalog is a CLI-managed Git repository with an explicit identity and a declarative manifest of unit identities and Git sources. It embeds no unit sources, requires no submodules, and pins no versions. A catalog entry never overrides the unit manifest's identity. Recommended (provisional): a JSONC manifest named `registry.jsonc`. | The catalog manifest schema and file name, catalog identity syntax, whether local paths are permitted, how unit references are validated at registration and synchronization, how a mismatch between a catalog entry and the referenced manifest is reported, and whether catalogs may reference other catalogs. | Registry | [catalogs](registries-and-discovery.md#catalogs--approved) |
| OC-33 | Synchronization is explicit, validates before committing, preserves the previous valid state on failure, and skips unchanged sources. Removal deletes only the registry relationship, never user repositories, and does not arbitrarily discard historical artifacts. | Synchronization granularity (individual sources, whether one failing source blocks others, per-source reporting), how a unit removed from a catalog is reflected, and how removal affects existing lock state and compositions that reference the removed source. | Registry | [synchronization](registries-and-discovery.md#synchronization--approved), [removal](registries-and-discovery.md#removal--approved) |

## Resolved contracts

| ID | Resolution | Context |
|----|------------|---------|
| OC-24 | **Superseded** by the registry model. The entry asked how units are discovered inside a directory grouping unit repositories. Grouping directories are no longer sources: units are registered individually or declared explicitly by a catalog, and a directory is never scanned to discover units. | [registered sources](registries-and-discovery.md#registered-sources--approved), [ADR 0003](../adr/0003-managed-local-registry.md) |

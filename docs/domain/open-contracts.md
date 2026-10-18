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
| OC-13 | Whether Alpha provides a remote push operation, and its command or flag. Approved: local publication and pushing are distinct, and `unit publish` never pushes commits or tags as a side effect. | Owner | [publication](authoring-workflow.md#publication--approved) |
| OC-14 | Owner syntax (character set) and boundary rules for both identity segments: leading, trailing, or consecutive hyphens, and length limits. These rules apply to catalog identities too, which use the same `@owner/name` convention. | Foundation | [identity](units.md#identity--approved), [catalogs](registries-and-discovery.md#catalogs--approved) |
| OC-15 | SemVer range dialect, and handling of pre-release and build metadata. Recommended (provisional) starting point: Cargo-like SemVer requirement semantics. | Foundation | [versioning](units.md#versioning--approved) |
| OC-16 | Policy for choosing among several versions that satisfy every range. It must be deterministic. Approved: existing lock state takes precedence over fresh resolution. Recommended (provisional): preserve satisfying locks, and otherwise prefer the highest compatible stable published version across the full transitive constraint set. | Foundation | [resolution](dependencies.md#resolution--approved), [version selection](dependencies.md#version-selection--recommended-provisional) |
| OC-18 | Behavior when a `src/` file has the same path as a file a dependency provides. | Foundation | [unit contents](units.md#src-unit-files) |
| OC-19 | Whether an amendment can delete an entire dependency file, or only change content. | Foundation | [amendments](amendments-and-composition.md#amendments--approved) |
| OC-21 | Whether `options` may target only direct dependencies or also transitive ones. | Foundation | [options](parameters-and-options.md#rules--approved) |
| OC-22 | Exact parameter required/default rules, for example whether a parameter without a default is implicitly required. | Foundation | [parameters](parameters-and-options.md#declaration--approved) |
| OC-26 | Publication edge cases: untracked files and the clean-state requirement, an existing tag that already points at the validated content, annotated vs lightweight tags (recommended (provisional): consider annotated tags), and whether a new version must be greater than earlier published versions. | Foundation | [publication](authoring-workflow.md#publication--approved) |

## Approved semantics with open details

| ID | Approved (normative) | Open detail | Origin | Context |
|----|----------------------|-------------|--------|---------|
| OC-2 | Amendments are declared in the manifest, with provenance and exact base. No earlier experimental syntax, such as `extends` or a `merges` field, is canonical. | Manifest schema and syntax for amendment declarations. | Owner | [amendments](amendments-and-composition.md#amendments--approved) |
| OC-3 | Amendments are full-file copies under `amendments/` that preserve provenance and exact base. | On-disk layout and naming of copies, and how provenance metadata is stored. | Owner | [amendments](amendments-and-composition.md#amendments--approved) |
| OC-4 | List identity rules may be needed where identity cannot be inferred safely. Identity is never guessed. | Manifest schema for list identity hints. | Owner | [merge semantics](amendments-and-composition.md#structured-and-unknown-formats) |
| OC-5 | Parameters affect file contents only. Rendering is deterministic. `own.*` is reserved for the amending author's context. Originals and amendments are rendered against compatible base contexts. Conditional content must not silently discard changes. | Template engine and syntax, escaping, non-text files, the exact rendering protocol, and the contents of `own.*`. | Owner | [rendering](parameters-and-options.md#rendering--approved-direction-details-open), [template contexts](amendments-and-composition.md#template-contexts--approved) |
| OC-6 | Required parameters without defaults must be supplied explicitly for development builds. | The mechanism for supplying them. | Owner | [build](authoring-workflow.md#build--approved) |
| OC-7 | The authoring operations (`init`, `amend`, `check`, `build`, `update`, `publish`), the registry command responsibilities (`registry create`, `add`, `remove`, `list`, `info`, `sync`, `doctor`), and discovery (`search`, `info`, with a machine-readable form) and their intent. `unit init` establishes the unit's Git repository. There are no commands solely for adding or removing catalog entries. | Exact grammar and semantics of every command, flag, and argument, including how `unit init` establishes the Git repository (new or existing repository, a target inside another repository, an initial commit), how `registry info` relates to the top-level `info`, and what `registry doctor` does beyond diagnosis. | Owner | [commands](authoring-workflow.md#commands--conceptual), [initialization](authoring-workflow.md#initialization--approved), [registry commands](registries-and-discovery.md#registry-commands--approved) |
| OC-8 | Versions and content are identifiable and verifiable. Working trees, published versions, the registry index, snapshots, and lock state are distinct. Snapshots preserve the provenance amendments need. Snapshots and lock state support deterministic reuse and reconciliation. Existing lock state takes precedence over fresh resolution, and lock state is independent of registry synchronization. Snapshots required by existing locks and for reconciliation are preserved. Historical content is never silently replaced. Recommended (provisional): content-addressed snapshots, and no automatic garbage collection of historical snapshots in Alpha. | Packaging, cache, snapshot, and lock-state formats and locations, including how content identity is computed and how long historical snapshots are retained. | Owner | [integrity](registries-and-discovery.md#integrity-and-snapshots--approved) |
| OC-9 | `unit check` verifies that every amendment's historical base is available. | Detailed behavior and diagnostics when historical Git objects (amendment bases, published versions) or registered sources are unavailable, for example a repository unreachable from another machine. | Owner | [amendments](amendments-and-composition.md#amendments--approved) |
| OC-10 | Copies are the authoring interface and operations the internal engine. Merge semantics as specified: deterministic, no silent loss, conflicts reported, conservative handling of unknown formats. | Production merge algorithms and format adapter selection. | Owner | [merge semantics](amendments-and-composition.md#merge-semantics--approved) |
| OC-11 | A unit with no effective contribution (files, amendments, or meaningful dependency configuration) is invalid for publication. This is not a "`src/` must contain files" rule. | The precise effective-contribution (no-op) algorithm. | Owner | [check](authoring-workflow.md#check--approved) |
| OC-17 | `unit init` provides a local, offline manifest JSON Schema referenced through `$schema`. Assemblr's own validation is authoritative. | The schema's file name and location, whether it is tracked in Git, and when it is regenerated. | Foundation | [editor support](manifest.md#editor-support--approved) |
| OC-20 | `unit amend` searches recursively by filename. Zero matches fail. Several matches are ambiguous and fail, listing the paths. An explicit relative path disambiguates. | Which file tree the search covers: the dependency's own files, or its composed output including files from its own dependencies. | Foundation | [amend](authoring-workflow.md#amend--approved) |
| OC-23 | *Revised by the registry model.* One global local registry, administered through the CLI. Unit repositories and catalogs are registered explicitly, validated as untrusted input, and incorporated atomically. Identical references to the same identity may be deduplicated. Different references to the same identity conflict. Two references are never assumed to denote the same repository: no URL normalization, shared-history, or common-commit heuristics, and no global repository identifier. Changing an identity's reference is a conflict, not a migration. There is no source priority and no automatic conflict resolution. | Whether any explicit resolution mechanism exists beyond removing or correcting a source, including whether the earlier mechanisms (explicit source selection, owner-to-source mappings) remain part of the model. Needs owner confirmation. | Foundation | [registration](registries-and-discovery.md#registration--approved), [source conflicts](registries-and-discovery.md#source-conflicts--approved) |
| OC-25 | Every unit is a Git repository. Tags are `v<version>` and must match the manifest version at the tagged commit exactly. A version tag whose manifest declares a different version is invalid. Different content for the same identity and published version, for example a moved tag, is an integrity conflict and is never trusted. | How invalid tags and integrity conflicts are reported, and how tags that are not of the form `v<version>` are treated. Whether such failures reject a whole operation or only exclude the affected versions is OC-33. | Foundation | [Git-based units](registries-and-discovery.md#units-are-git-repositories--approved), [integrity](registries-and-discovery.md#integrity-and-snapshots--approved) |
| OC-27 | Updates are explicit. Changing a resolved version and changing a declared range are separate concerns. Ranges are never widened automatically. | The command surface for each (see OC-7). Version choice within a range follows OC-16. | Foundation | [update](authoring-workflow.md#update--approved) |
| OC-28 | `unit build` performs the applicable native validations before generating output. | Exactly which `check` validations apply to `build`, for example whether the effective-contribution check also gates build. | Foundation | [build](authoring-workflow.md#build--approved) |
| OC-29 | Discovery is metadata-based, not semantic or AI-driven. `info` content is specified. Human and machine output derive from the same data. | Exact `search` matching rules and deterministic result ordering. | Foundation | [discovery](registries-and-discovery.md#discovery-and-information--approved) |
| OC-32 | A catalog is a Git repository whose `registry.jsonc` has a `name` (an `@owner/name` catalog identity, in a namespace separate from units) and a `units` map from expected unit identities to plain-string Git references. HTTPS, SSH, and local references are permitted. Relative references resolve against the catalog root. The referenced manifest must declare the catalog's identity for it. A catalog duplicates no versions, dependencies, parameters, or hashes, and needs no SemVer or release publication. Entries may be edited directly and are validated through registration or synchronization. Remote catalogs synchronize from the Git default branch, and the index records the exact catalog commit. | Whether `registry.jsonc` may contain fields beyond `name` and `units`. How conflicting catalog identities are handled. Which revision is read for a local catalog, and how uncommitted local catalog changes are treated. How identical references are recognized when relative references come from different catalogs. | Registry | [catalogs](registries-and-discovery.md#catalogs--approved), [synchronization](registries-and-discovery.md#synchronization--approved) |
| OC-33 | Registration and synchronization validate untrusted sources and never execute their code. Synchronization updates catalogs and units, validates the complete proposed index before replacing the active one, keeps the previous valid state when a required validation fails, removes dropped catalog references from discovery, preserves snapshots required by locks, and never modifies consumer projects or lock files. Removal deletes only the registry relationship. | Which validation failures are required, and so reject the whole registration or synchronization, and which only exclude the affected versions. The earlier rule that an inconsistent tag fails only the affected version still stands, and the two must be reconciled. Whether synchronization can target individual sources. | Registry | [registration](registries-and-discovery.md#registration--approved), [synchronization](registries-and-discovery.md#synchronization--approved) |

## Resolved contracts

| ID | Resolution | Context |
|----|------------|---------|
| OC-24 | **Superseded** by the registry model. The entry asked how units are discovered inside a directory grouping unit repositories. Grouping directories are no longer sources: units are registered individually or declared explicitly by a catalog, and a directory is never scanned to discover units. | [registered sources](registries-and-discovery.md#registered-sources--approved), [ADR 0003](../adr/0003-managed-local-registry.md) |
| OC-30 | **Resolved.** Unit dependencies resolve exclusively from published, tagged versions available through the managed registry. During `unit build`, the authored unit uses its working tree and its dependencies use published versions. There are no unpublished dependency overrides and no working-tree dependency consumption in Alpha. | [resolution](dependencies.md#resolution--approved), [build](authoring-workflow.md#build--approved) |
| OC-31 | **Resolved.** `unit publish` updates the index automatically when the unit is already registered, never registers an unknown unit, and never pushes. A failed index update after a successful publication is reported as a partial outcome, and `registry sync` recovers. `registry sync` discovers changes made outside local publication. `search`, `info`, `unit check`, `unit build`, and `unit update` never fetch implicitly, and a missing published version is reported as needing synchronization. | [index freshness](registries-and-discovery.md#index-freshness-and-fetching--approved), [publication](authoring-workflow.md#publication--approved) |

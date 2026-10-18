# Open Contracts

This register lists every point in the domain documents that is not fully
decided. It has two parts:

- **[Unresolved contracts](#unresolved-contracts)**: product questions with no
  decision yet. Each needs an owner decision before it is implemented.
- **[Approved semantics with open details](#approved-semantics-with-open-details)**:
  the behavior is approved and normative, and implementations must conform to
  it. What remains open is the listed detail, such as syntax, formats,
  algorithms, or edge cases. A detail that changes user-visible behavior, syntax,
  or file formats still needs an owner decision. A purely internal
  implementation choice is an engineering decision, recorded in
  [`docs/adr/`](../adr/README.md) when significant.

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

## Unresolved contracts

| ID | Question | Origin | Context |
|----|----------|--------|---------|
| OC-1 | **Consumer-project lifecycle.** Creating consumer projects; persisting compositions; adding, removing, and updating units; consumer-specific configuration; consumer CLI UX. **Reserved** for an owner-led design discussion. | Owner | [README](README.md#reserved-the-consumer-project-lifecycle) |
| OC-12 | Ordering policy for insertions at the same anchor. Approved so far: when the order may be significant, a conflict is preferred to an arbitrary order. The exact policy needs further validation. | Owner | [ordering](amendments-and-composition.md#ordering) |
| OC-13 | Whether remote push is part of the Alpha publication workflow, and how. Local publication and pushing are distinct operations. | Owner | [publication](authoring-workflow.md#publication--approved) |
| OC-14 | Owner syntax (character set) and boundary rules for both identity segments: leading, trailing, or consecutive hyphens, and length limits. | Foundation | [identity](units.md#identity--approved) |
| OC-15 | SemVer range dialect, and handling of pre-release and build metadata. | Foundation | [versioning](units.md#versioning--approved) |
| OC-16 | Policy for choosing among several versions that satisfy every range (for example, highest compatible). It must be deterministic. | Foundation | [resolution](dependencies.md#resolution--approved) |
| OC-18 | Behavior when a `src/` file has the same path as a file a dependency provides. | Foundation | [unit contents](units.md#src-unit-files) |
| OC-19 | Whether an amendment can delete an entire dependency file, or only change content. | Foundation | [amendments](amendments-and-composition.md#amendments--approved) |
| OC-21 | Whether `options` may target only direct dependencies or also transitive ones. | Foundation | [options](parameters-and-options.md#rules--approved) |
| OC-22 | Exact parameter required/default rules, for example whether a parameter without a default is implicitly required. | Foundation | [parameters](parameters-and-options.md#declaration--approved) |
| OC-26 | Publication edge cases: untracked files and the clean-state requirement, an existing tag that already points at the validated content, annotated vs lightweight tags, and whether a new version must be greater than earlier published versions. | Foundation | [publication](authoring-workflow.md#publication--approved) |

## Approved semantics with open details

| ID | Approved (normative) | Open detail | Origin | Context |
|----|----------------------|-------------|--------|---------|
| OC-2 | Amendments are declared in the manifest, with provenance and exact base. No earlier experimental syntax, such as `extends` or a `merges` field, is canonical. | Manifest schema and syntax for amendment declarations. | Owner | [amendments](amendments-and-composition.md#amendments--approved) |
| OC-3 | Amendments are full-file copies under `amendments/` that preserve provenance and exact base. | On-disk layout and naming of copies, and how provenance metadata is stored. | Owner | [amendments](amendments-and-composition.md#amendments--approved) |
| OC-4 | List identity rules may be needed where identity cannot be inferred safely. Identity is never guessed. | Manifest schema for list identity hints. | Owner | [merge semantics](amendments-and-composition.md#structured-and-unknown-formats) |
| OC-5 | Parameters affect file contents only. Rendering is deterministic. `own.*` is reserved for the amending author's context. Originals and amendments are rendered against compatible base contexts. Conditional content must not silently discard changes. | Template engine and syntax, escaping, non-text files, the exact rendering protocol, and the contents of `own.*`. | Owner | [rendering](parameters-and-options.md#rendering--approved-direction-details-open), [template contexts](amendments-and-composition.md#template-contexts--approved) |
| OC-6 | Required parameters without defaults must be supplied explicitly for development builds. | The mechanism for supplying them. | Owner | [build](authoring-workflow.md#build--approved) |
| OC-7 | The authoring operations (`init`, `amend`, `check`, `build`, `update`, `publish`) and discovery (`search`, `info`, with a machine-readable form) and their intent. | Exact grammar and semantics of every command, flag, and argument, including whether `unit init` creates or requires a Git repository. | Owner | [commands](authoring-workflow.md#commands--conceptual) |
| OC-8 | Versions and content are identifiable and verifiable. Snapshots and lock state support deterministic reuse and reconciliation. Mutable sources never silently change a resolved snapshot. | Packaging, cache, snapshot, and lock-state formats and locations, including how content identity is computed. | Owner | [integrity](registries-and-discovery.md#integrity-and-snapshots--approved) |
| OC-9 | `unit check` verifies that every amendment's historical base is available. | Detailed behavior and diagnostics when historical Git objects (amendment bases, published versions) are unavailable. | Owner | [amendments](amendments-and-composition.md#amendments--approved) |
| OC-10 | Copies are the authoring interface and operations the internal engine. Merge semantics as specified: deterministic, no silent loss, conflicts reported, conservative handling of unknown formats. | Production merge algorithms and format adapter selection. | Owner | [merge semantics](amendments-and-composition.md#merge-semantics--approved) |
| OC-11 | A unit with no effective contribution (files, amendments, or meaningful dependency configuration) is invalid for publication. This is not a "`src/` must contain files" rule. | The precise effective-contribution (no-op) algorithm. | Owner | [check](authoring-workflow.md#check--approved) |
| OC-17 | `unit init` provides a local, offline manifest JSON Schema referenced through `$schema`. Assemblr's own validation is authoritative. | The schema's file name and location, whether it is tracked in Git, and when it is regenerated. | Foundation | [editor support](manifest.md#editor-support--approved) |
| OC-20 | `unit amend` searches recursively by filename. Zero matches fail. Several matches are ambiguous and fail, listing the paths. An explicit relative path disambiguates. | Which file tree the search covers: the dependency's own files, or its composed output including files from its own dependencies. | Foundation | [amend](authoring-workflow.md#amend--approved) |
| OC-23 | Several local registries may be configured. Explicit source selection has priority, owner-to-source mappings may disambiguate, and otherwise a unit found in several sources is ambiguous. There is no silent precedence. | Where and how registries, explicit source selection, and owner-to-source mappings are configured. | Foundation | [registries](registries-and-discovery.md#registry-configuration--approved) |
| OC-24 | A registry may point to a single unit or to a directory grouping separately maintained unit repositories. | How units are discovered inside a grouping directory, for example depth and nested repositories. | Foundation | [registries](registries-and-discovery.md#registry-configuration--approved) |
| OC-25 | Tags are `v<version>` and must match the manifest version exactly. A version tag whose manifest declares a different version is invalid. | How invalid tags are reported, and how tags that are not of the form `v<version>` are treated. | Foundation | [Local Git](registries-and-discovery.md#local-git-registry--approved) |
| OC-27 | Updates are explicit. Changing a resolved version and changing a declared range are separate concerns. Ranges are never widened automatically. | The command surface for each (see OC-7). Version choice within a range follows OC-16. | Foundation | [update](authoring-workflow.md#update--approved) |
| OC-28 | `unit build` performs the applicable native validations before generating output. | Exactly which `check` validations apply to `build`, for example whether the effective-contribution check also gates build. | Foundation | [build](authoring-workflow.md#build--approved) |
| OC-29 | Discovery is metadata-based, not semantic or AI-driven. `info` content is specified. Human and machine output derive from the same data. | Exact `search` matching rules and deterministic result ordering. | Foundation | [discovery](registries-and-discovery.md#discovery-and-information--approved) |

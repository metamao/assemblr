# Open Contracts

This register lists every point in the domain documents that is not fully
decided. It has three parts:

- **[Unresolved contracts](#unresolved-contracts)**: product questions with no
  decision yet. Each needs an owner decision to be settled. An
  implementation may proceed on a recorded provisional decision only where
  [AGENTS.md §1.1](../../AGENTS.md#11-decisions-autonomy-by-default-escalation-by-exception)
  allows it.
- **[Approved semantics with open details](#approved-semantics-with-open-details)**:
  the behavior is approved and normative, and implementations must conform to
  it. What remains open is the listed detail, such as syntax, formats,
  algorithms, or edge cases. A detail that changes user-visible behavior, syntax,
  or file formats still needs an owner decision. A purely internal
  implementation choice is an engineering decision, recorded in
  [`docs/adr/`](../adr/README.md) when significant.
- **[Resolved contracts](#resolved-contracts)**: entries settled or superseded
  by a decision, kept so their numbers stay traceable.

Each entry is a `### OC-n` heading, linkable as `open-contracts.md#oc-n`,
followed by a list with the fields of its part: **Question**, **Origin**, and
**Context** for an unresolved contract; **Approved (normative)**, **Open
detail**, **Origin**, and **Context** for approved semantics with open
details; **Resolution** and **Context** for a resolved contract. Moving an
entry to another part means moving its heading and changing its fields.

Implementations must not resolve any entry by assumption. Two things are
not resolution by assumption: following a default the owner has accepted as
**Recommended (provisional)**, and a **provisional decision** an agent makes
within its delegated authority (AGENTS.md §1.1) and records in the
autonomous decision log. A provisional decision is implemented behavior, not
an approved contract: the entry stays open, the owner may revise the
decision, and a decision that sets product direction, changes an approved
statement, or commits a compatibility or integrity guarantee is never
provisional and needs the owner. When a decision
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
- **Consumer**: a gap or contradiction found while incorporating the approved
  consumer-project and catalog-metadata contracts. It needs owner confirmation
  that it is a real question.
- **Readiness**: a gap found while assessing readiness to begin core
  implementation. The owner has confirmed it is a real question that does not
  block the first core implementation increment.
- **Implementation**: a gap found while implementing an increment. The owner
  has accepted the cited provisional default for Alpha, and the entry stays
  open until the owner settles it.

How the status of a statement affects implementation:

- **Approved** decisions are binding. Implementations must conform to them.
- A **Recommended (provisional)** default cited by an entry has been accepted
  by the owner to guide Alpha implementation, which may follow it. It is not a
  final decision: the entry stays open until the owner settles it, and having
  a recommended implementation never closes an entry by itself.
- The **open details** of an entry stay open until decided. They are settled
  by the owner when they affect user-visible behavior, syntax, or file formats,
  and by engineering otherwise. Until then an agent may implement a recorded
  provisional decision under AGENTS.md §1.1.
- A recommendation the owner has **not** accepted, for example from research
  or a review, is not a requirement.

## Unresolved contracts

### OC-12

- **Question:** Ordering policy for insertions at the same anchor. Approved so
  far: when the order may be significant, a conflict is preferred to an
  arbitrary order. The exact policy needs further validation. **Provisional
  (D-6)** for the JSON merge: members several amenders add at one place are
  placed in a fixed order by amender identity rather than reported as a
  conflict, on the premise that JSON object member order carries no meaning.
  The premise has a counterexample: Node reads the conditions of a
  `package.json` `exports` in order, so the invented order between two units'
  insertions can decide which condition applies. **Known limitation, entry
  stays open:** the merge does not guarantee the meaning of member order where
  a JSON consumer gives it one, when several units amend one object. The owner
  decided to keep the generic behavior for now, with no technology-specific
  rules in the engine, no new manifest metadata, and no broad conflict for
  independent contributions, until real projects give evidence. The line merge
  of text files follows the same stance (**Provisional (D-20)**): insertions
  several amenders make at one position are all kept in amender-identity order
  and reported, because two units appending sections to one guidance file is
  the common case. Across provider versions, insertions the provider's release
  and an older amendment make at one position are both kept with the
  provider's first, in composition and in `unit update` (owner decision after
  the second decision-log review, #70; see [across provider
  versions](amendments-and-composition.md#across-provider-versions--approved)).
  **Known limitation for text, entry stays open:** a clean line merge does not
  guarantee semantic compatibility in formats where the order or repetition of
  lines decides the outcome, such as `.env`, `CODEOWNERS`, and `.gitignore`;
  the order of the amenders' identities can decide which line takes effect
  (see [text
  files](amendments-and-composition.md#text-files--approved-direction-details-provisional-d-20)).
  No such format is refused for it (owner decision D-27). A copy's reorder is
  reported as a warning to its author and the merge keeps the provider's
  order; the warnings and tests describe the behavior and do not resolve the
  entry.
- **Origin:** Owner
- **Context:** [ordering](amendments-and-composition.md#ordering)

### OC-48

- **Question:** **File-system semantics of contributed files**: executable
  bits; symbolic links in unit sources; binary content; line endings, for
  example a checkout that converts line endings; paths that differ only in
  case, which collide on case-insensitive file systems; and paths reserved for
  Assemblr's own files, such as `assemblr.jsonc` and the lockfiles. These
  affect content identity (OC-8) and reconciliation. Approved meanwhile: two
  units' `src/` files delivered at the same path are a path collision (a
  template is delivered without its `.assemblr.tmpl` suffix), and so is a file
  whose delivered path is a directory of another file's, within one unit or
  across units; `src/` paths follow the portable path rules in [unit
  files](units.md#src-unit-files). Approved (owner, 2026-10-23): paths that
  differ only in case are also path collisions, because a case-insensitive
  file system would otherwise lose one of the files silently. Approved: paths
  that differ only in Unicode normalization collide too, alone or combined
  with case differences. The comparison decomposes canonically (NFD), maps
  each character to its single-character lowercase and then uppercase, and
  decomposes again; it is not Unicode's canonical caseless matching, which
  uses full case folding (see [unit files](units.md#src-unit-files)).
  Compatibility equivalents do not collide. Approved for `unit build`: how a
  working tree's symbolic links, special files, and executable bits are read
  (see [build details](authoring-workflow.md#build-details--approved)).
  Approved in the [owner review of 2026-10-23](https://github.com/metamao/assemblr/issues/34#issuecomment-6358234799) (D-10): files are
  written byte for byte with the executable bit as composed, never through a
  symbolic link; `assemblr.jsonc`, `assemblr-lock.json`, and `.assemblr/` are
  reserved paths a unit cannot deliver. Approved (D-15) for
  reconciliation: a project file that differs from its
  base or the incoming file only by CRLF line endings counts as that content
  and keeps its line endings; the executable bit is reconciled like content,
  and where the file system has none the project's mode is read as the base's.
  Which delivered files are unbased is decided by the project's `.gitignore`
  files only, never by one machine's `.git/info/exclude` or global excludes; a
  file only those ignore, and that the base would store for the first time, is
  refused with the way out named (D-16, which revises D-11 and D-15,
  approved). **Still open: binary content.**
- **Origin:** Readiness
- **Context:** [unit files](units.md#src-unit-files), [file
  contributions](consumer-projects.md#file-contributions--approved)

### OC-49

- **Question:** **Lock portability across machines.** What happens when the
  source a consumer lock records for a unit differs from the source registered
  for that identity on another machine, and what a lock records for a unit
  obtained through a relative catalog reference. Unavailable sources and Git
  objects are OC-9.
- **Origin:** Readiness
- **Context:** [lock](consumer-projects.md#lock-and-reconciliation-state--approved),
  [source conflicts](registries-and-discovery.md#source-conflicts--approved)

## Approved semantics with open details

### OC-4

- **Approved (normative):** List identity rules may be needed where identity
  cannot be inferred safely. Identity is never guessed.
- **Open detail:** Manifest schema for list identity hints. **Provisional
  (D-8)** for the JSON merge: lists are compared whole, so no identity is
  inferred and changes inside one list are never merged independently;
  identical whole-list changes are one change. Decided by the owner for the
  alpha: lists of scalars several units amend are joined, provider items
  first, then each amender's in identity order, without duplicates; which
  changes are joinable, and whole lists in consumer reconciliation, are
  **Provisional (D-38)**.
- **Origin:** Owner
- **Context:** [merge
  semantics](amendments-and-composition.md#structured-and-unknown-formats)

### OC-5

- **Approved (normative):** Parameters affect file contents only. Rendering is
  deterministic, and a pure function of one file's source and its context. A
  unit file's context is the resolved parameter values of the unit that
  provides it. `own` is reserved for the amending author's context, is not a
  parameter name, and is defined only when an amendment copy is rendered,
  where it holds the amending unit's own resolved parameter values. An
  undefined reference is a rendering error, never empty text. Templates are
  selected explicitly by the `.assemblr.tmpl` suffix, matched exactly at the
  end of the name, and delivered without it; every other file is delivered
  byte for byte; the suffix is a rendering switch, not a file class, and an
  amendment copy keeps the provider's path, so it inherits the outcome. The
  grammar is `{{ name }}` references, `{{ own.name }}`, and the escape `{{
  "{{" }}`, a subset of Jinja's; every other construct opening with `{{`,
  `{%`, or `{#` is an error, never passed through. Strings render raw,
  integers in decimal, and booleans as `true` or `false`, with no quoting or
  format-specific escaping; inserted text is never rendered again; text
  outside references is copied byte for byte. A template must be UTF-8.
  Rendering errors are collected across files, ordered by delivered path and
  position, with file, line, and column, and they gate `unit build`. Originals
  and amendments are rendered against compatible base contexts.
- **Open detail:** Whether constructs beyond references, such as conditional
  content or filters, are ever supported, and with which engine; if
  conditional content is added, it must not silently discard changes.
- **Origin:** Owner
- **Context:** [rendering](parameters-and-options.md#rendering--approved),
  [templates](units.md#templates), [template
  contexts](amendments-and-composition.md#template-contexts--approved)

### OC-6

- **Approved (normative):** Required parameters without defaults must be
  supplied explicitly for development builds. Development build values
  configure only the unit being built; its dependencies are configured only
  through its own `options`. Decided by the owner (#62, decision log D-24): a
  parameter's `example` is the value a unit's build, check, and publication
  use for a required parameter without a default; it is not a default and
  never a consumer value (see
  [examples](parameters-and-options.md#examples--approved) and [development
  values](authoring-workflow.md#build-details--approved)).
- **Open detail:** Building one unit under several configurations (for example
  `--value` flags); not offered.
- **Origin:** Owner
- **Context:** [build](authoring-workflow.md#build--approved)

### OC-7

- **Approved (normative):** The authoring operations (`init`, `amend`,
  `check`, `build`, `update`, `publish`), the registry command
  responsibilities (`registry create`, `add`, `remove`, `list`, `info`,
  `sync`, `doctor`), catalog refresh, discovery (`search`, `info`, with a
  machine-readable form), and the consumer operations (`init`, `add`,
  `remove`, `compose`, and continuing or aborting a pending composition) and
  their intent. `unit init` establishes the unit's Git repository. There are
  no commands solely for adding or removing catalog entries. `add` accepts one
  unit per invocation and records a caret (`^`) requirement by default. The
  CLI is non-interactive by default. For local unit repositories, the registry
  grammar is `registry add <path>`, `registry list`, `registry list --json`
  (with a [documented
  schema](registries-and-discovery.md#implemented-grammar--approved)),
  `registry remove <identity>`, and `registry sync`; adding a registered
  reference again changes nothing. `unit build` takes no arguments and builds
  the unit whose repository's top level is the current directory, without
  searching parent directories. `unit amend <file> [--unit <identity>]`
  searches the direct dependencies' `src/` at their locked versions for a file
  name, or exactly for a path, matching a template with or without its suffix,
  restricted to one dependency by `--unit`, and reports several matches rather
  than choosing (see [amend
  details](authoring-workflow.md#amend-details--approved)). `unit update
  [<identity>...]` updates every locked dependency, or the named ones, direct
  or transitive, to the newest published versions their requirements allow; it
  requires a lockfile, never changes a range, and never fetches (see [update
  details](authoring-workflow.md#update-details--approved)). `unit check` and
  `unit publish` take no arguments and run at the repository's top level;
  `unit check` runs every validation a build runs plus the publication checks
  and writes nothing, and `unit publish` tags the committed tree at `HEAD` and
  updates the index for the registered source (**Provisional (D-3, D-4)**; see
  [check details](authoring-workflow.md#check-details--approved-d-1-d-3) and
  [publication details](authoring-workflow.md#publication-details--approved-d-3-d-4)).
  `unit init <identity> [<directory>] [--description <text>]` establishes the
  repository (initialized, adopted at its top level, nested inside a
  repository that is not a unit's, refused inside a unit's), writes the
  manifest, the schema, and `.gitignore`, never overwrites a manifest, and
  never commits (**Provisional (D-5, D-33)**; see [initialization
  details](authoring-workflow.md#initialization-details--provisional-d-5-d-33)). `assemblr
  init [<directory>]` writes an empty declaration and nothing else, and
  `assemblr compose` composes the declaration into a project composed for the
  first time (**Provisional (D-9)**; see [initialization
  details](consumer-projects.md#initialization-details--provisional-d-9) and
  [composition details](consumer-projects.md#composition-details--provisional-d-10-to-d-12)).
  `assemblr add <identity>[@<requirement>]` and `assemblr remove <identity>`
  edit the declaration and compose it: `add` records a caret requirement on
  the resolved version when none is given and keeps the requirement of a unit
  that is selected already, `remove` takes the selection and its `options`
  entry and refuses a unit that is not an explicit selection (**Provisional
  (D-17)**; see [add and remove
  details](consumer-projects.md#add-and-remove-details--provisional-d-17)). `assemblr
  update [<identity>...]` is decided by the owner (#63, decision log D-23; see
  [update](consumer-projects.md#update--approved)). `assemblr info
  <identity>[@<version>] [--json]` describes a registered unit's newest stable
  or named version from the index and its repository, offline (**Provisional
  (D-19)**; see the [implemented
  grammar](registries-and-discovery.md#implemented-grammar--approved)).
  `registry add` also takes a remote reference (`https://`, `ssh://`,
  `file://`, or `user@host:path`), stored as given, and `add` and `sync`
  fetch remote sources into copies that every other operation reads offline
  (**Provisional (D-35)**).
  `assemblr search <term>... [--json]` lists the registered units whose
  identity or newest stable description contains every term, offline
  (**Provisional (D-32)**, under OC-29).
  `registry create <catalog> [<directory>]` creates a catalog's repository
  and `registry.jsonc`, and `catalog refresh` regenerates its index
  (**Provisional (D-36)**). `registry add` registers a catalog as well,
  `registry sync` synchronizes catalogs, and `registry remove --catalog
  <identity>` removes one (**Provisional (D-39)**).
- **Open detail:** Exact grammar and semantics of every other command, flag,
  and argument, including: how `registry info` relates to the top-level
  `info`, and whether unit inspection is spelled `info` or `unit info`; what
  `registry doctor` does beyond diagnosis; how catalog operations (`registry
  create`, catalog refresh) are grouped; how `add` takes parameter values
  (`--option <name>=<value>`, **Provisional (D-34)**, its values read by the
  parameter's type by owner decision). The grammar of `unit init` (its
  default directory, **Provisional (D-33)**), `init`, `compose`, `add`, and
  `remove`, and `--verbose` on `compose`, `add`, `remove`, and `update`
  (**Provisional (D-31)**), stays open until the owner closes it. Approved in
  the [owner review of 2026-10-23](https://github.com/metamao/assemblr/issues/34#issuecomment-6358234799): `unit check` and `unit publish` (D-3), and
  `compose --continue` and `--abort` (D-18).
- **Origin:** Owner
- **Context:** [commands](authoring-workflow.md#commands--conceptual),
  [initialization](authoring-workflow.md#initialization--approved), [registry
  commands](registries-and-discovery.md#registry-commands--approved), [catalog
  refresh](registries-and-discovery.md#catalog-refresh--approved),
  [composition commands](consumer-projects.md#composition-commands--approved)

### OC-8

- **Approved (normative):** Versions and content are identifiable and
  verifiable. Working trees, published versions, the registry index,
  snapshots, and lock state are distinct. Snapshots preserve the provenance
  amendments need. Snapshots and lock state support deterministic reuse and
  reconciliation. Existing lock state takes precedence over fresh resolution,
  and lock state is independent of registry synchronization. Snapshots
  required by existing locks and for reconciliation are preserved. Historical
  content is never silently replaced. A consumer project has a versioned,
  CLI-managed lockfile, distinct from `assemblr.jsonc` and suitable for
  version control, that records exact versions, provenance, and a verifiable
  identity of the reconciliation base. That base stays recoverable or
  verifiably reconstructible without the consumer's Git history, and a
  reconstructed base is checked against its recorded identity. A published
  version's content identity is the root tree of its tagged commit, which
  covers the entire repository snapshot, and the commit is its provenance.
  Recommended (provisional): content-addressed snapshots, and no automatic
  garbage collection of historical snapshots in Alpha. Decided by the owner
  for consumer projects (#41): the consumer lockfile is `assemblr-lock.json`
  with the unit lockfile's entries plus a versioned `project` reconciliation
  state (base identity, managed and unbased paths); the base's identity is
  `sha256-v1:` over a canonical versioned listing of mode, content hash, and
  path; the base is stored in the project under `.assemblr/`, tracked,
  content-addressed, marked binary for Git; the stored base is authoritative
  and verified before use, and a missing or unverifiable base is an explicit
  failure (see [the lockfile and the stored
  base](consumer-projects.md#the-lockfile-and-the-stored-base--approved)).
- **Open detail:** Regeneration of a missing base from the locked inputs as a
  verified fallback; retention of historical bases; the formats and locations
  of other caches and snapshots.
- **Origin:** Owner
- **Context:** [integrity](registries-and-discovery.md#integrity-and-snapshots--approved),
  [lock](consumer-projects.md#lock-and-reconciliation-state--approved)

### OC-9

- **Approved (normative):** `unit check` verifies that every amendment's
  historical base is available. Accepted provisionally: `unit build` fails
  when it cannot read the registered source of any identity it can reach
  through the dependencies of any published version, including versions no
  requirement allows. Resolution's decision order depends on the dependencies
  of all of them (OC-16), so an unreadable source can change which versions
  are chosen.
- **Open detail:** Detailed behavior and diagnostics when historical Git
  objects (amendment bases, published versions) or registered sources are
  unavailable, for example a repository unreachable from another machine.
  Whether resolution may disregard sources reachable only through versions no
  requirement allows, which needs a decision order that ignores those
  versions.
- **Origin:** Owner
- **Context:** [amendments](amendments-and-composition.md#amendments--approved)

### OC-10

- **Approved (normative):** Copies are the authoring interface and operations
  the internal engine. Merge semantics as specified: deterministic, no silent
  loss, conflicts reported, conservative handling of unknown formats. Consumer
  projects reconcile three-way (reconciliation base, current, incoming).
  Verifying an amendment's base is distinct from applying the amendment to the
  version a composition resolved: a resolved version other than the base is
  normal, and the changes derived against the verified base are applied to it,
  with incompatible changes reported. Decided by the owner (#33) for unit
  composition: a value-level N-way merge deriving operations from each copy
  against its verified base; JSON and JSONC first, with a format-preserving
  writer and the written text checked against the merged value; identical
  contributions are one change and different changes to one value conflict
  without precedence; no layered overrides; lists compared whole, never merged
  inside, except appends to lists of scalars, which are joined (owner, alpha;
  **Provisional (D-38)** for the details); other formats refused with several amenders, except UTF-8 text,
  which is combined by line (owner direction after the reference scenario;
  **Provisional (D-20)**; see [text
  files](amendments-and-composition.md#text-files--approved-direction-details-provisional-d-20)),
  YAML, TOML, and INI included (owner decision on #92, replacing their
  refusal), while XML and files that are not text stay refused
  (**Provisional (D-37)**);
  cross-version application (OC-52) and update reconciliation (OC-53) deferred
  then, and decided on #59 (decision log D-22); deterministic results per
  engine version, semantic rather than byte-identical stability across
  releases (see [the merge
  engine](amendments-and-composition.md#the-merge-engine--approved)).
- **Open detail:** Further format adapters (TOML, YAML, which are merged by
  line until one exists), and whether Git takes part in unit composition. The consumer side is **Provisional (D-13)**:
  per-path three-way reconciliation against the last materialized composition,
  without Git; the value merge for a JSON or JSONC file both sides changed,
  onto the project's text, and a line-level merge for a UTF-8 text file
  (**Provisional (D-21)**); every other disagreement refused or left to the
  pending-resolution state (see [reconciliation
  details](consumer-projects.md#reconciliation-details--provisional-d-13-to-d-15)).
- **Origin:** Owner
- **Context:** [merge
  semantics](amendments-and-composition.md#merge-semantics--approved),
  [reconciliation](consumer-projects.md#reconciliation--approved)

### OC-23

- **Approved (normative):** *Revised by the registry model.* One global local
  registry, administered through the CLI. Unit repositories and catalogs are
  registered explicitly, validated as untrusted input, and incorporated
  atomically. Identical references to the same identity may be deduplicated.
  Different references to the same identity conflict. Two references are never
  assumed to denote the same repository: no URL normalization, shared-history,
  or common-commit heuristics, and no global repository identifier. Changing
  an identity's reference is a conflict, not a migration. There is no source
  priority and no automatic conflict resolution.
- **Open detail:** Whether any explicit resolution mechanism exists beyond
  removing or correcting a source, including whether the earlier mechanisms
  (explicit source selection, owner-to-source mappings) remain part of the
  model, and whether a registered local source can be relocated without
  removing and registering it again. Needs owner confirmation.
- **Origin:** Foundation
- **Context:** [registration](registries-and-discovery.md#registration--approved),
  [source conflicts](registries-and-discovery.md#source-conflicts--approved)

### OC-25

- **Approved (normative):** Every unit is a Git repository. Tags are
  `v<version>` and must match the manifest version at the tagged commit
  exactly. A version tag whose manifest declares a different version is
  invalid. Different content for the same identity and published version, for
  example a moved tag, is an integrity conflict and is never trusted.
  Published versions are recognized only through tags of the form `v<SemVer>`;
  other tags are not published versions; an invalid version never enters the
  usable version index and is reported explicitly. Every excluded version tag
  is reported with its reason on stderr, and tags outside the convention, such
  as `v1.0`, are ignored and not mentioned. Registration and synchronization
  reject a source whose published versions include two of equal SemVer
  precedence, such as versions that differ only in build metadata.
- **Open detail:** How integrity conflicts and other rejections are presented
  beyond that. Whether publication rejects a new version that differs from a
  published one only in build metadata.
- **Origin:** Foundation
- **Context:** [Git-based
  units](registries-and-discovery.md#units-are-git-repositories--approved),
  [integrity](registries-and-discovery.md#integrity-and-snapshots--approved)

### OC-27

- **Approved (normative):** Updates are explicit. Changing a resolved version
  and changing a declared range are separate concerns. Ranges are never
  widened automatically. Changing resolved versions is `unit update`, which
  never changes a range (see OC-7).
- **Open detail:** The command surface for changing a range (see OC-7).
  Version choice within a range follows OC-16.
- **Origin:** Foundation
- **Context:** [update](authoring-workflow.md#update--approved)

### OC-29

- **Approved (normative):** Discovery is metadata-based, not semantic or
  AI-driven. `info` content is specified. Human and machine output derive from
  the same data.
- **Open detail:** Exact `search` matching rules and deterministic result
  ordering. **Provisional (D-32)** and implemented by `search`: every
  whitespace-separated term must occur as a substring, ignoring case by
  Unicode's default lowercase mapping, in the unit's identity or in the
  description of its newest stable version (the version `info` describes);
  results are in identity order (see the [implemented
  grammar](registries-and-discovery.md#implemented-grammar--approved)). The
  entry stays open until the owner closes it.
- **Origin:** Foundation
- **Context:** [discovery](registries-and-discovery.md#discovery-and-information--approved)

### OC-32

- **Approved (normative):** A catalog is a Git repository whose
  `registry.jsonc` has a `name` (an `@owner/name` catalog identity, in a
  namespace separate from units) and a `units` map from expected unit
  identities to plain-string Git references. HTTPS, SSH, and local references
  are permitted. Relative references resolve against the catalog root. The
  referenced manifest must declare the catalog's identity for it.
  `registry.jsonc` duplicates no versions, dependencies, parameters, or
  hashes, and indexing adds no fields to it. A catalog needs no SemVer or
  release publication. Entries may be edited directly and are validated
  through registration or synchronization. Remote catalogs synchronize from
  the Git default branch, and the index records the exact catalog commit.
- **Open detail:** Whether `registry.jsonc` may contain fields beyond `name`
  and `units`. How conflicting catalog identities are handled. Which revision
  is read for a local catalog, and how uncommitted local catalog changes are
  treated. How identical references are recognized when relative references
  come from different catalogs. Implemented provisionally for catalog
  refresh (D-36): only `name`, `units`, and `$schema` are accepted, and
  refresh reads the working tree. Implemented provisionally for
  registration (D-39): a local catalog is read as committed at `HEAD`, a
  remote catalog may declare only remote references, and references are
  compared after a local catalog's relative paths are resolved.
- **Origin:** Registry
- **Context:** [catalogs](registries-and-discovery.md#catalogs--approved),
  [synchronization](registries-and-discovery.md#synchronization--approved)

### OC-33

- **Approved (normative):** Registration and synchronization validate
  untrusted sources and never execute their code. Synchronization updates
  catalogs and units, validates the complete proposed index before replacing
  the active one, keeps the previous valid state when a source-level
  validation fails, removes dropped catalog references from discovery,
  preserves snapshots required by locks, and never modifies consumer projects
  or lock files. Removal deletes only the registry relationship. Versions of
  catalog-managed units are discovered through the catalog's metadata index.
  An invalid version is excluded from the usable index and reported, and the
  operation continues. A source-level failure, such as an invalid or
  inaccessible repository, an invalid catalog manifest, or a source conflict,
  rejects the registration or synchronization atomically. For unit
  repositories, every check is
  [classified](registries-and-discovery.md#invalid-versions-and-failed-sources--approved):
  failures of a newly discovered tag, including invalid `src/` content and the
  self-contained validation of published amendments (copies and declarations
  correspond, and the lockfile locks each amended provider at its base), are
  version-level; unreadable repositories, conflicting identities, versions of
  equal precedence, and integrity violations of indexed versions are
  source-level; an indexed version whose tag disappears, moves, or becomes
  invalid rejects the whole synchronization, across the registry. Checks that
  depend on other sources, such as verifying a base's content against the
  registry record, belong to the build that composes the amendment.
- **Open detail:** The classification of the checks of catalogs and their
  metadata indexes. Whether synchronization can target individual sources. How
  the index update of `unit publish`, and a unit registered both individually
  and through a catalog, interact with versions the catalog's index does not
  yet list. Implemented provisionally (D-39): every catalog problem is
  source-level; `unit publish` indexes nothing for a unit only catalogs
  manage; a unit registered both ways is indexed with every valid tag.
- **Origin:** Registry
- **Context:** [registration](registries-and-discovery.md#registration--approved),
  [synchronization](registries-and-discovery.md#synchronization--approved)

### OC-36

- **Approved (normative):** `assemblr.jsonc` is the editable JSONC declaration
  of explicit `dependencies` and their `options`, with manifest-consistent
  semantics and no `params`. Editing assistance comes from local registry
  metadata. Recommended (provisional): a static JSON Schema plus a locally
  generated, project-aware schema, through existing editor tooling, with no
  language server or editor extension. The CLI is authoritative.
- **Open detail:** The full schema of `assemblr.jsonc`, its initial contents,
  and fields beyond `dependencies` and `options`, whose serialization is
  approved (see [manifest
  serialization](manifest.md#serialization-of-dependencies-options-params-and-amendments--approved));
  `assemblr.jsonc` has no `params`; schema file names and locations; how the
  project-aware schema is generated and refreshed. **Provisional (D-9)**:
  `init` writes `dependencies` and `options` as empty maps, and the reader
  accepts `$schema`; no schema file is written yet.
- **Origin:** Owner
- **Context:** [project
  configuration](consumer-projects.md#project-configuration--approved),
  [editing
  assistance](consumer-projects.md#editing-assistance--approved-direction-details-open)

### OC-37

- **Approved (normative):** Unresolved conflicts put the project in an
  explicit pending-resolution state. Conflicts are resolved in place with the
  user's own tools, with access to reconciliation base, current, and incoming
  context. Continue validates and finalizes. Abort restores the pre-operation
  state. No commits or Git history changes. Planning failures leave the
  project unmodified. Failed application preserves enough state for safe
  recovery. Strict multi-file atomicity is not promised.
- **Open detail:** How conflicts are presented (for example, markers or file
  layout) and how the three versions are exposed; how resolution is marked
  complete; which operations are allowed while resolution is pending; the
  snapshot, journal, and restore mechanisms; and recovery after an interrupted
  application. Approved in the [owner review of 2026-10-23](https://github.com/metamao/assemblr/issues/34#issuecomment-6358234799) (D-12, D-14, D-18,
  and the fixes of #72 and #73): a journal in
  `.assemblr/` records the files being created, replaced, and removed, with
  their previous content kept under `.assemblr/previous/`, and the next
  `compose` completes an interrupted composition whose lock is in place and
  reverts any other to the previous content. A
  journal whose format number or members this version does not know is refused
  and left as it is (#73, ADR 0027). The journal also records the declaration
  that `add` and `remove` rewrite, with its previous text, and a revert
  restores it with the files (**Provisional (D-17)**). For text conflicts: `compose --continue` and `--abort`; each conflicted file
  stays at its path holding the project's, the base's, and the composition's
  content between diff3-style markers; the journal keeps the lock and the
  marked files and waits; only `--continue` and `--abort` run while it waits;
  `--continue` needs no line starting `<<<<<<< current`, `|||||||base`, or
  `>>>>>>> incoming` left and writes the lock; `--abort` restores the project,
  the declaration included, and removes the aborted base and the objects only
  it names from the store (#72). A conflict of any other kind, mode,
  deleted-versus-changed, removed-but-edited, non-text, or unbased, still
  refuses the composition whole. A text file both sides changed is merged by
  line, and a conflict is marked only around the lines that disagree
  (**Provisional (D-21)**); a structured file's conflict keeps whole-file
  markers, and marking only the colliding values of a JSON file is deferred.
- **Origin:** Owner
- **Context:** [conflict
  resolution](consumer-projects.md#conflict-resolution--approved),
  [preparation and
  failure](consumer-projects.md#preparation-and-failure--approved)

### OC-38

- **Approved (normative):** Consumer update operations. Updates are explicit
  and distinct from the minimal changes `add` requires. Decided by the owner
  (#63, decision log D-23): `assemblr update [<identity>...]` moves locked
  versions within the declared requirements, all or the named units (selected
  or transitive), keeps the others unless forced and reports what moved, never
  edits `assemblr.jsonc`, records the lock even when the files are identical,
  and reconciles like `compose` with its journal and `--continue`/`--abort`
  (see [update](consumer-projects.md#update--approved)).
- **Open detail:** Changing requirements in bulk, updating past the
  requirements (`--latest`), a dry run, and automatic updates.
- **Origin:** Owner
- **Context:** [version
  resolution](consumer-projects.md#version-resolution--approved), [explicit
  updates](dependencies.md#explicit-updates--approved)

### OC-39

- **Approved (normative):** A published unit satisfies its direct
  dependencies' required parameters through defaults, its `options`, or
  mappings from its own public parameters. Mappings reference only declared
  parameters with compatible types and allowed values. `unit publish`
  validates these obligations across the resolved graph, through the
  dependency chain. Decided by the owner (#62, decision log D-24): a mapping
  is written `{ "from": "<own parameter>" }` as an option value and is a
  strict requirement carrying the source's effective value, defaults included,
  with no conversion; values are decided from the dependents to the
  dependencies through any number of levels; compatible values from several
  paths agree and different ones are an options conflict that names the
  mapping (see
  [mappings](parameters-and-options.md#satisfying-dependency-parameters--approved)).
- **Open detail:** Transformations of values, name-based propagation, and
  options on transitive units, which are not offered.
- **Origin:** Consumer
- **Context:** [satisfying dependency
  parameters](parameters-and-options.md#satisfying-dependency-parameters--approved)

### OC-40

- **Approved (normative):** A catalog publishes an Assemblr-generated,
  deterministic metadata index, derived from its declared sources and their
  published manifests, which stay authoritative. Catalog refresh regenerates
  it and is a maintainer operation, suitable for automation, with no mandatory
  central service, webhook, or back-reference.
- **Open detail:** The index's file name, serialization, storage location, and
  hash representation; whether a catalog without a published index can be
  registered; how refresh reports partial failures. Implemented
  provisionally (D-36, D-39, [ADR 0030](../adr/0030-catalog-index.md)): a
  catalog with no committed index cannot be registered;
  `registry-index.json` at the catalog's root, JSON with sorted keys, Git
  commit and tree ids, and an invalid version excluded with a warning while
  any source-level failure rejects the whole refresh.
- **Origin:** Owner
- **Context:** [catalog metadata
  index](registries-and-discovery.md#catalog-metadata-index--approved)

### OC-41

- **Approved (normative):** Catalog-generated metadata is untrusted.
  Registration and synchronization validate its structure, identities,
  provenance, internal consistency, and correspondence with authoritative
  published unit data, to the extent the source-trust guarantees require.
  Neither unverified trust nor full source downloads on every ordinary
  registry operation is acceptable.
- **Open detail:** The verification and caching strategy that achieves these
  guarantees efficiently. Implemented provisionally (D-39, [ADR
  0031](../adr/0031-catalog-registration.md)): registration checks each
  listed manifest's structure, identity, and version; the commit, tree, and
  manifest are checked against the unit's repository when its content is
  first retrieved, and every build checks them again.
- **Origin:** Owner
- **Context:** [metadata ingestion and
  trust](registries-and-discovery.md#metadata-ingestion-and-trust--approved)

### OC-42

- **Approved (normative):** Source declarations, validated metadata, and
  complete unit content are distinct. Discovery and inspection work from
  metadata. Complete content is retrieved and cached when composition requires
  it, not when a catalog is registered.
- **Open detail:** Which operations retrieve content on demand, and how that
  relates to the rule that `search`, `info`, `unit check`, `unit build`, and
  `unit update` never fetch implicitly; Git partial-fetch and caching
  mechanisms. **Provisional (D-35)** for remote unit
  repositories: `registry add` and `registry sync` fetch every published tag
  and `HEAD` into a full bare copy, with no partial fetch, and every other
  operation reads the copy and never fetches. **Decided by the owner
  (2026-10-23, in session, #95)** for catalogs: registering or synchronizing
  a catalog reads only its metadata, and a unit's content is fetched when
  composition first needs it, so `add` and `compose` may fetch.
  **Provisional (D-39)**: any operation that reads a version the copy lacks
  fetches it, including `update` and the dependencies read by `unit build`,
  `unit check`, `unit update`, and `unit amend`; fetching the content of an indexed commit is not read as
  fetching remote changes, since it cannot change the index or a
  resolution.
- **Origin:** Consumer
- **Context:** [metadata and
  content](registries-and-discovery.md#metadata-and-content--approved)

### OC-43

- **Approved (normative):** Git is a requirement: every unit is a Git
  repository, and Assemblr uses Git's object and tag infrastructure.
- **Open detail:** The minimum supported Git version and related portability
  constraints.
- **Origin:** Owner
- **Context:** [Git-based
  units](registries-and-discovery.md#units-are-git-repositories--approved)

### OC-44

- **Approved (normative):** Composition reconciles every contributed file
  alike, never silently overwrites the consumer's content, and `init` modifies
  no existing application files.
- **Open detail:** How a composition treats a path that already holds content
  Assemblr did not materialize, so that no reconciliation base exists for it:
  for example, whether it is a conflict resolved in the pending-resolution
  state or an error reported before anything is applied. **Provisional
  (D-10)** for the first `compose`: such a path is refused before anything is
  applied, naming every path; the same holds for a new file of a reconciled
  composition (**Provisional (D-13)**).
- **Origin:** Consumer
- **Context:** [file
  contributions](consumer-projects.md#file-contributions--approved)

### OC-52

- **Approved (normative):** An amendment's base and copy are rendered against
  compatible contexts before changes are derived, and a composition applies
  the derived changes to the provider's file at the resolved version. Decided
  by the owner (#59, decision log D-22): the base and the copy are rendered
  with one context, the composition's values for the provider's parameters,
  plus `own`; the changes are applied to the resolved file rendered with the
  same context; a parameter a template names must be declared at the resolved
  version with the same kind of value as at the base, or the composition fails
  and names it, with no fallback to a default of the base version (see [across
  provider
  versions](amendments-and-composition.md#across-provider-versions--approved)).
- **Open detail:** A parameter whose meaning changes while its name and kind
  stay is not detected.
- **Origin:** Owner
- **Context:** [template
  contexts](amendments-and-composition.md#template-contexts--approved)

### OC-53

- **Approved (normative):** `unit update` reconciles the source templates of a
  unit's amendments from the old base to the new version, together with the
  declared base versions and the lock, and never leaves a silent partial
  update. Decided by the owner (#59, decision log D-22): an unchanged file is
  carried over; a changed text file is rebased by a line merge on the source
  templates, neither side taking precedence except that insertions both make
  at one position keep the provider's first (#70), and the copy, the declared
  base, and the lock change together; a collision, a file with no merge, or
  non-text content writes nothing and is reported with each collision's lines
  (see [update details](authoring-workflow.md#update-details--approved)).
- **Open detail:** A marker-based way to complete or abandon an update with
  conflicts is not offered for units; revisit with evidence.
- **Origin:** Foundation
- **Context:** [update](authoring-workflow.md#update--approved)

## Resolved contracts

### OC-1

- **Resolution:** **Resolved.** The consumer-project lifecycle is approved:
  `init`, the editable `assemblr.jsonc` declaration, `add`, `remove`, and
  `compose`, explicit and transitive dependencies, non-interactive parameter
  handling, file contributions, reconciliation, Git-like conflict resolution,
  and the lockfile. Its open details are tracked as OC-7, OC-8, OC-10, OC-36
  to OC-38, OC-44, and OC-47 to OC-49.
- **Context:** [consumer-projects.md](consumer-projects.md)

### OC-14

- **Resolution:** **Resolved.** Owners and unit names use lowercase ASCII
  letters, digits, and hyphens. Each segment is non-empty, begins and ends
  with a letter or digit, and has no consecutive hyphens. Malformed identities
  are rejected, never normalized. There is no identity-specific length limit.
  Catalog identities follow the same rules.
- **Context:** [identity](units.md#identity--approved)

### OC-2

- **Resolution:** **Resolved.** Amendments are declared in the manifest's
  top-level `amendments` map, keyed by the amended path, each value naming the
  direct dependency that provides the file as `unit` and the exact base
  version as `base`, which must equal that dependency's locked version. A copy
  without a declaration and a declaration without a copy are errors. No
  earlier experimental syntax, such as `extends` or a `merges` field, is
  canonical.
- **Context:** [layout and
  declaration](amendments-and-composition.md#layout-and-declaration),
  [serialization](manifest.md#serialization-of-dependencies-options-params-and-amendments--approved)

### OC-3

- **Resolution:** **Resolved.** An amendment copy lives at
  `amendments/<path>`, where `<path>` is the file's path in the provider's
  `src/`, under the same path rules, and the amending unit's own `src/` must
  not hold that path. Provenance is stored in the manifest declaration (the
  target identity and exact base version, OC-2), and the base's content
  identity is the amending unit's lockfile entry for that version (OC-46).
- **Context:** [layout and
  declaration](amendments-and-composition.md#layout-and-declaration),
  [amendments](units.md#amendments-modified-dependency-files)

### OC-18

- **Resolution:** **Resolved.** Two units that provide a file at the same path
  through their `src/` directories cause a path collision error, whether or
  not one depends on the other. No unit wins by order or priority, and the
  files are never merged. Modifying a dependency's file requires an amendment.
  The remaining file-system questions are OC-48.
- **Context:** [unit files](units.md#src-unit-files)

### OC-19

- **Resolution:** **Resolved.** Amendments change a file's content only. They
  delete, rename, and create no files, and they do not change a file's
  executable bit.
- **Context:** [layout and
  declaration](amendments-and-composition.md#layout-and-declaration)

### OC-20

- **Resolution:** **Resolved**, revised by the owner on #100 (2026-10-23).
  An amendment's base is layered: the provider's `src/` file at the exact
  version the amender is locked to, as the units below the amender compose
  it, which is the file as it stands in the amender's `build/`. A unit can
  change what a unit below it added or changed; siblings combine side by
  side, and a unit above conflicting siblings resolves the conflict in its
  copy, from a base that marks it. Provenance records the provider's
  identity and exact base version in the manifest, and the lockfile entries
  for the provider and the amenders below identify the base's content. `unit
  amend` searches the `src/` trees of every unit in the lock state,
  recursively by filename: zero matches fail, several matches fail listing
  the paths, and an explicit relative path or `--unit` disambiguates. An
  amendment targets any unit below the amender. The marker shapes, the
  pending state's writes, and the reconstruction of a published amender's
  base are provisional (D-41, D-42, D-43).
- **Context:** [the base](amendments-and-composition.md#the-base),
  [amend](authoring-workflow.md#amend--approved)

### OC-21

- **Resolution:** **Resolved.** A unit's `options` target only its direct
  dependencies. Transitive dependencies' required configuration is satisfied
  through the chain. Incompatible requirements remain options conflicts,
  without precedence.
- **Context:** [options rules](parameters-and-options.md#rules--approved),
  [satisfying dependency
  parameters](parameters-and-options.md#satisfying-dependency-parameters--approved)

### OC-22

- **Resolution:** **Resolved.** A parameter without a default is required, and
  one with a default is optional. There is no `required` property. A missing
  required value is an explicit validation error.
- **Context:** [parameter
  declaration](parameters-and-options.md#declaration--approved)

### OC-24

- **Resolution:** **Superseded** by the registry model. The entry asked how
  units are discovered inside a directory grouping unit repositories. Grouping
  directories are no longer sources: units are registered individually or
  declared explicitly by a catalog, and a directory is never scanned to
  discover units.
- **Context:** [registered
  sources](registries-and-discovery.md#registered-sources--approved), [ADR
  0003](../adr/0003-managed-local-registry.md)

### OC-28

- **Resolution:** **Resolved.** `unit build` is gated by manifest validity,
  identity, and version; parameter and options consistency, including
  dependency parameters satisfied through the chain; dependency resolution;
  `src/` path validity and path collisions, on delivered paths; amendment
  validity, as the [build
  details](authoring-workflow.md#build-details--approved) list it; and the
  rendering of the templates of the unit and its dependencies, amended copies
  included. The effective-contribution check (OC-11) and a clean working tree
  do not gate a build.
- **Context:** [build details](authoring-workflow.md#build-details--approved)

### OC-30

- **Resolution:** **Resolved.** Unit dependencies resolve exclusively from
  published, tagged versions available through the managed registry. During
  `unit build`, the authored unit uses its working tree and its dependencies
  use published versions. There are no unpublished dependency overrides and no
  working-tree dependency consumption in Alpha.
- **Context:** [resolution](dependencies.md#resolution--approved),
  [build](authoring-workflow.md#build--approved)

### OC-31

- **Resolution:** **Resolved.** `unit publish` updates the index automatically
  when the unit is already registered, never registers an unknown unit, and
  never pushes. A failed index update after a successful publication is
  reported as a partial outcome, and `registry sync` recovers. `registry sync`
  discovers changes made outside local publication. `search`, `info`, `unit
  check`, `unit build`, and `unit update` never fetch implicitly, and a
  missing published version is reported as needing synchronization.
- **Context:** [index
  freshness](registries-and-discovery.md#index-freshness-and-fetching--approved),
  [publication](authoring-workflow.md#publication--approved)

### OC-34

- **Resolution:** **Superseded.** The entry asked how files in a consumer
  project are classified as derived or scaffold. That classification is
  withdrawn: every contributed file follows one lifecycle, and changes are
  applied when they can be reconciled safely against the reconciliation base
  and the consumer's current content. The remaining case of a path that
  already holds content Assemblr did not materialize continues as OC-44.
- **Context:** [file
  contributions](consumer-projects.md#file-contributions--approved), [unit
  files](units.md#src-unit-files)

### OC-35

- **Resolution:** **Superseded.** The entry asked for the interface of a
  separate scaffold update. No separate scaffold-update operation is part of
  the model: changes reach every contributed file through ordinary composition
  and reconciliation.
- **Context:** [file
  contributions](consumer-projects.md#file-contributions--approved)

### OC-11

- **Resolution:** **Resolved.** A unit is effective when it delivers a `src/`
  file or declares a dependency; an amendment copy identical to the provider's
  file at its base is redundant and reported; an option equal to the locked
  version's default is not reported (D-1). Owner decision in the [owner review
  of
  2026-10-23](https://github.com/metamao/assemblr/issues/34#issuecomment-6358234799).
- **Context:** [check
  details](authoring-workflow.md#check-details--approved-d-1-d-3)

### OC-13

- **Resolution:** **Resolved.** Alpha provides no push operation. `unit
  publish` never pushes; after a new publication its result suggests `git push
  --follow-tags`, which pushes the commit and its annotated tag but no other
  local tag. Owner decision in the [owner review of
  2026-10-23](https://github.com/metamao/assemblr/issues/34#issuecomment-6358234799).
- **Context:** [publication
  details](authoring-workflow.md#publication-details--approved-d-3-d-4)

### OC-15

- **Resolution:** **Resolved.** Requirements follow Cargo's SemVer requirement
  syntax and semantics, and every comparator names its operator: a bare
  version such as `1.2.3` is refused, because Cargo reads it as `^1.2.3` while
  npm, pip, and Poetry read it as exactly `1.2.3`. Wildcards such as `*` and
  `1.*` are explicit. `add <unit>@1.2.3` records `^1.2.3`. Pre-releases and
  build metadata are as approved. Owner decision in the [owner review of
  2026-10-23](https://github.com/metamao/assemblr/issues/34#issuecomment-6358234799).
- **Context:** [versioning](units.md#versioning--approved), [resolution
  policy](dependencies.md#resolution-policy--approved)

### OC-16

- **Resolution:** **Resolved.** Satisfying locks are preserved, and otherwise
  the highest compatible stable published version across the full transitive
  constraint set is chosen, with the details of the [resolution
  policy](dependencies.md#resolution-policy--approved): candidates that
  conflict, depend on a missing or ambiguous identity, or close a cycle are
  passed over, and the unit decided first keeps the higher version. Selection
  is deterministic and not promised to be optimal. The recommended default,
  accepted for Alpha, is approved. Owner decision in the [owner review of
  2026-10-23](https://github.com/metamao/assemblr/issues/34#issuecomment-6358234799).
- **Context:** [resolution
  policy](dependencies.md#resolution-policy--approved)

### OC-17

- **Resolution:** **Resolved.** `manifest.schema.json` at the repository's top
  level, tracked, created when missing and refreshed by running `unit init`
  again; `unit check` and `unit publish` never compare it, since Assemblr's
  validation is authoritative (D-2). Owner decision in the [owner review of
  2026-10-23](https://github.com/metamao/assemblr/issues/34#issuecomment-6358234799).
- **Context:** [editor support](manifest.md#editor-support--approved)

### OC-26

- **Resolution:** **Resolved.** Committing everything must change nothing,
  with untracked unignored files refused by name; a tag of the version at
  `HEAD` means already published and exits zero; tags are annotated with the
  author's identity; a new version need not be greater than earlier ones, and
  a version of equal precedence is refused (D-3). Owner decision in the [owner
  review of
  2026-10-23](https://github.com/metamao/assemblr/issues/34#issuecomment-6358234799).
- **Context:** [publication
  details](authoring-workflow.md#publication-details--approved-d-3-d-4)

### OC-45

- **Resolution:** **Resolved.** A parameter definition has `type` (`"string"`,
  `"integer"`, `"boolean"`), `description`, an optional `default`, an optional
  `allowed`, valid only for strings, and an optional `example` (#62). The
  default accepted for Alpha is approved. Owner decision in the [owner review
  of
  2026-10-23](https://github.com/metamao/assemblr/issues/34#issuecomment-6358234799).
- **Context:** [parameter
  definitions](manifest.md#parameter-definitions-and-manifest-text--approved)

### OC-46

- **Resolution:** **Resolved.** `unit publish` never writes the unit lockfile;
  it requires the lockfile to be committed and to record the current
  resolution, so its writers are `unit build` and `unit update` (D-4). Owner
  decision in the [owner review of
  2026-10-23](https://github.com/metamao/assemblr/issues/34#issuecomment-6358234799).
- **Context:** [unit lock
  state](authoring-workflow.md#unit-lock-state--approved)

### OC-47

- **Resolution:** **Resolved.** A conflict between units' contributions in a
  consumer composition, such as an amendment that cannot be applied to the
  resolved provider version, is a composition error: the project is not
  modified, and the consumer can select another version, for example the base
  with `assemblr add <unit>@=<base>` (D-10, D-13). It is never a
  pending-resolution state, because the consumer is not the author of those
  units. Owner decision in the [owner review of
  2026-10-23](https://github.com/metamao/assemblr/issues/34#issuecomment-6358234799).
- **Context:** [composition
  details](consumer-projects.md#composition-details--provisional-d-10-to-d-12)

### OC-50

- **Resolution:** **Resolved.** Parameter names start with an ASCII letter and
  continue with ASCII letters, digits, or underscores; `own` is reserved. The
  default accepted for Alpha is approved. Owner decision in the [owner review
  of
  2026-10-23](https://github.com/metamao/assemblr/issues/34#issuecomment-6358234799).
- **Context:** [parameter
  definitions](manifest.md#parameter-definitions-and-manifest-text--approved)

### OC-51

- **Resolution:** **Resolved.** `//` and `/* */` comments and trailing commas
  after a value are accepted; other JSON5 extensions are rejected; duplicate
  keys are rejected; integers are 64-bit signed; descriptions must not be
  empty or only whitespace. The default accepted for Alpha is approved. Owner
  decision in the [owner review of
  2026-10-23](https://github.com/metamao/assemblr/issues/34#issuecomment-6358234799).
- **Context:** [manifest
  text](manifest.md#parameter-definitions-and-manifest-text--approved)

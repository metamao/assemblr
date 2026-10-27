# 0038: Remove the `registry.jsonc` catalogs

- **Status:** Accepted
- **Date:** 2026-10-27

## Context

A catalog was a Git repository whose `registry.jsonc` mapped unit identities
to repository URLs, with a generated `registry-index.json` of their versions
(ADRs 0005, 0030, 0031, 0033). Its only real use, the goldenrelic units,
showed the costs:

- Every real change touched several units. Each one needed a PR and a tag,
  and the catalog needed a PR of its own.
- The catalog's author fixes each URL. Registering the catalog by SSH when
  its entries use HTTPS fails, and since Assemblr sets `GIT_CONFIG_GLOBAL` to
  the null device for reads, no `insteadOf` can fix it.

The owner decided on 2026-10-27 to remove these catalogs. The name "catalog"
now goes to a repository that holds several units published together (#127).
Assemblr is not released, so no published catalog depends on the old ones.

## Decision

1. **No catalog commands.** `catalog init`, `add`, `remove`, and `refresh`
   are gone, and so are their scaffolds. `registry remove --catalog` is gone
   too. `registry add` of a repository with `registry.jsonc` and no
   `manifest.jsonc` fails like any repository without a manifest: no
   identity at `HEAD`. A hint says a unit's repository has `manifest.jsonc`
   at its root.
2. **One kind of registered unit.** The registry index no longer records
   catalogs, or units only catalogs manage. Every unit is registered
   directly. `registry remove` always removes the registration, and `unit
   publish` no longer reports a unit that only catalogs manage.
3. **Index format 4.** It has no `direct` field and no `catalogs`. Formats 1
   to 3 are still read and written as 4 on the next change (ADR 0027). A
   format 3 index that lists a catalog, or a unit with `direct` set to
   `false`, is refused, with what to do: move the index aside and register
   each unit's repository again. It is never read silently, since its
   catalog-managed units would otherwise lose the source that justified
   them.
4. **`registry list --json`** drops the `direct` and `catalogs` fields and
   the top-level `catalogs`. The schema only promised that fields may be
   added. This removal is a compatibility break of the output, accepted
   because nothing released depends on it (**Provisional (D-51)**, like the
   handling of format 3 above).
5. **Content on demand stays.** Fetching a version's files when it is first
   read was introduced for catalog units (D-39). ADR 0037 extends it to
   every remote unit, so it remains.

## Consequences

- The open contracts about catalogs (OC-32, OC-40, OC-41) no longer apply.
- The agent bench drops the three catalog scenarios (s14 to s16). Scenarios
  for the new catalogs come with #127.
- A machine that used the goldenrelic catalog registers its units again,
  from the new catalog repository, once #127 and the migration land.

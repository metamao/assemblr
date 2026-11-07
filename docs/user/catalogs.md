# Keep several units in one repository

Units that change together can share a repository, a catalog, and be
published together at one version. Put `catalog.jsonc` at the top of a Git
repository:

```jsonc
{ "name": "@acme/web", "version": "1.0.0" }
```

Then `assemblr unit init @acme/api` there creates the unit in `units/api/`.
Its manifest has no `version`: every unit has the catalog's. One unit uses
another of the same catalog with `"catalog"` instead of a version range:

```jsonc
"dependencies": { "@acme/api": "catalog" }
```

`assemblr unit build` and `assemblr unit check` at the top level cover every
unit, and in `units/api/` only that one. `assemblr unit publish`, at the top
level, tags the whole catalog as `v1.0.0`. Others register it once, and get
every unit in it.

A unit can amend another unit's file in the same catalog: run
`assemblr unit amend <file>` in its directory. The copy's base is kept in
`.assemblr/` at the top level, so commit that directory too. When the other
unit's file changes, `assemblr unit build` merges the change into the copy,
and stops with conflict markers in the copy when both changed the same
lines.

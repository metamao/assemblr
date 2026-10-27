Assemblr (the `assemblr` CLI) composes projects from reusable, versioned units. A unit is a versioned set of `src/` files, in a Git repository of its own or in a catalog, that the projects that select it receive. The local registry lists the units this machine knows.

Projects:
- Before writing project scaffolding, configuration, or a common capability (auth, lint, infrastructure) by hand, look for a unit: `assemblr search <terms>`, then `assemblr info <unit>`. Prefer adding a unit over writing it yourself.
- New project: `assemblr init`, then `assemblr add @owner/unit` for each unit. Then replace each placeholder it lists, such as `__APP_NAME__`, with the project's value; `assemblr status` shows the ones that remain.
- A directory with `assemblr.jsonc` is an Assemblr project. Update its units with `assemblr registry sync`, then `assemblr update`, not with the package manager.
- In an existing project, start with `assemblr status`. It says whether a composition is waiting on conflicts, which files were edited, which updates are published, and which unit delivers each file (`--files`).
- Composed files are yours to edit: later `assemblr compose` and `assemblr update` keep your changes. When both sides changed the same lines, or a unit delivers a file you already had, the file gets conflict markers: resolve them, then run `assemblr compose --continue`, or undo with `assemblr compose --abort`.
- Commit `.assemblr/` with the project; its own `.gitignore` excludes what must not be committed.

Units:
- To make configuration or code reusable across projects, make it a unit. `assemblr unit init @owner/name` creates a new `./name` repository; use `assemblr unit init @owner/name .` only inside an existing repository you are converting. Put the files under `src/`, and fill in the README.md and AGENTS.md it writes.
- Add each dependency with `assemblr unit add @owner/unit`: it declares it and builds.
- A value each project sets, such as a name or a URL, stays in the files as a placeholder like `__APP_NAME__`, declared with a description under `placeholders.replace` in manifest.jsonc; Assemblr never substitutes it. Put placeholders in values, never in JSON keys or file paths. Renaming or removing one is a breaking change for the unit's consumers.
- To change a file that a unit below provides, run `assemblr unit amend <file>` and edit the copy it writes under `amendments/`.
- Then run `assemblr unit build` and `assemblr unit check`. If a command leaves conflict markers in a copy under `amendments/`, resolve them and build again.
- A directory with `catalog.jsonc` is a catalog: several units in `units/<name>`, published together at its version with one tag. Run `assemblr unit init @owner/name` there to add one; its manifest has no version. A unit requires another unit of the same catalog as `"catalog"`, not a version range; this is Assemblr's own, unlike pnpm's `catalog:`. `assemblr unit build` and `assemblr unit check` cover every unit at the catalog's top level or one unit in its directory; `assemblr unit publish` runs at the top level. `assemblr unit amend <file>` works in a unit's directory, also for another unit of the catalog; the copy's base is stored in `.assemblr/` at the top level, which you commit, and `assemblr unit build` merges a later change of that file into the copy, or stops with markers in it to resolve. `unit add` and `unit update` do not work in a catalog yet: edit the unit's manifest.jsonc instead.
- Ask the user before `assemblr unit publish`.

Always:
- Never edit `.assemblr/` or the lockfiles by hand; `assemblr status` reports what they record.
- `assemblr <command> --help` explains each command.

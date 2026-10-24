Assemblr (the `assemblr` CLI) composes projects from reusable, versioned units. A unit is a Git repository whose `src/` files the projects that select it receive. The local registry lists the units this machine knows.

Projects:
- Before writing project scaffolding, configuration, or a common capability (auth, lint, infrastructure) by hand, look for a unit: `assemblr search <terms>`, then `assemblr info <unit>`. Prefer adding a unit over writing it yourself.
- New project: `assemblr init`, then `assemblr add @owner/unit --option name=value` for each unit.
- A directory with `assemblr.jsonc` is an Assemblr project. Update its units with `assemblr registry sync`, then `assemblr update`, not with the package manager.
- Composed files are yours to edit: later `assemblr compose` and `assemblr update` keep your changes. When both sides changed the same lines, or a unit delivers a file you already had, the file gets conflict markers: resolve them, then run `assemblr compose --continue`, or undo with `assemblr compose --abort`.
- Commit `.assemblr/` with the project; its own `.gitignore` excludes what must not be committed.

Units:
- To make configuration or code reusable across projects, make it a unit. `assemblr unit init @owner/name` creates a new `./name` repository; use `assemblr unit init @owner/name .` only inside an existing repository you are converting. Put the files under `src/`, and fill in the README.md and AGENTS.md it writes.
- Add each dependency with `assemblr unit add @owner/unit`: it declares it, maps its required parameters, and builds.
- To change a file that a unit below provides, run `assemblr unit amend <file>` and edit the copy it writes under `amendments/`.
- Then run `assemblr unit build` and `assemblr unit check`. If a command leaves conflict markers in a copy under `amendments/`, resolve them and build again.
- Ask the user before `assemblr unit publish`.

Catalogs:
- A catalog is a Git repository whose `registry.jsonc` lists unit repositories, so that another machine registers them all at once with `assemblr registry add <path or URL>`.
- `assemblr catalog init @owner/name` creates one. In its directory, `assemblr catalog add <URL>` adds a unit by its repository URL (never a local path) and `assemblr catalog remove @owner/unit` removes one; both update `registry-index.json` and commit. Fill in the README.md and AGENTS.md that `catalog init` writes and commit them, then `git push`.
- After publishing a unit version and pushing its tag, run `assemblr catalog refresh` in each catalog that lists the unit (it commits), then `git push`: until then, machines that use the catalog do not see the version.

Always:
- Never edit `.assemblr/` or the lockfiles by hand.
- `assemblr <command> --help` explains each command.

# Use units in a project

1. **Register the units** you want to use in your local registry, and the
   units they build on:

   ```sh
   assemblr registry add https://github.com/acme/rust-base.git
   assemblr registry add https://github.com/acme/rust-service.git
   ```

   Then `assemblr search rust` and `assemblr info @acme/rust-service` show
   what is available and which placeholders a unit's files hold.

2. **Create the project in a new, empty directory:**

   ```sh
   mkdir orders-api && cd orders-api
   git init
   assemblr init
   ```

   `assemblr init` creates `assemblr.jsonc` and an `AGENTS.md` for coding
   agents in the current directory. To adopt Assemblr in an existing
   project, run it at the project's root instead. If a unit later delivers a
   text file you already have, that file gets conflict markers for you to
   resolve. Any other file in the way stops the composition before anything
   is written, and the error names it.

3. **Add a unit.** Assemblr writes its files, and those of the units it
   builds on, into the project:

   ```sh
   assemblr add @acme/rust-service
   ```

   The result lists each placeholder left to replace, with its description
   and where it is, such as `__BINARY_NAME__` in `Dockerfile`. Replace each
   one with your value, such as `orders-api`. `assemblr status` shows the
   ones that remain.

4. **Commit** the files, including `assemblr-lock.json` and `.assemblr/`.
   From here on, edit the delivered files freely: they are yours.

5. **Stay up to date.** When the units publish new versions:

   ```sh
   assemblr registry sync    # learn about new versions
   assemblr status           # see available updates and your edited files
   assemblr update           # move to the newest versions allowed
   ```

**If a unit's change collides with yours,** the file gets conflict markers,
as in a Git merge. In a text file, changes in different places are both
kept, and only changes to the same lines collide. In a YAML, TOML, XML, or
INI file you edited, any change from the unit is a conflict. Edit the file
to resolve the markers, then run `assemblr compose --continue`. To undo the
whole composition instead, run `assemblr compose --abort`.

## Tips

- **Formatters.** When Assemblr edits `assemblr.jsonc` or `manifest.jsonc`,
  it follows the file's style, trailing commas included, so a file Prettier
  formatted stays formatted. Assemblr alone writes `assemblr-lock.json` and
  `.assemblr/`, like `package-lock.json`: list them in `.prettierignore`.
- **Never edit `.assemblr/` or `assemblr-lock.json` by hand.**
  `assemblr status` reports what they record.

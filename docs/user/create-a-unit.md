# Create a unit

The examples use the owner `acme`. Replace the names and URLs with
your own.

1. **Create it.** This makes a `rust-service/` directory with a manifest, a
   `src/` directory, a `README.md`, an `AGENTS.md`, and a Git repository:

   ```sh
   assemblr unit init @acme/rust-service
   cd rust-service
   ```

2. **Add your files** under `src/`. Each file is delivered to projects at the
   same path, so `src/Dockerfile` becomes the project's `Dockerfile`.

   A value each project sets, such as the binary's name, stays in the file
   as a placeholder like `__BINARY_NAME__`. Declare it in `manifest.jsonc`
   with a description of the value to write:

   ```jsonc
   "placeholders": {
     "replace": { "__BINARY_NAME__": "The binary's name, in kebab-case." }
   }
   ```

   Assemblr never fills a placeholder in. It delivers the file as it is and
   tells each project which placeholders it still has to replace. Put them
   in values, never in file names or JSON keys.

3. **Build on other units** (optional). Add a unit from your registry as a
   dependency, such as `@acme/rust-base`, then copy one of its files to
   change it:

   ```sh
   assemblr unit add @acme/rust-base
   assemblr unit amend Cargo.toml
   ```

   Edit the copy under `amendments/`. Assemblr merges your change into future
   versions of `@acme/rust-base`.

4. **Build and check it.** `build/` shows exactly what a project would
   receive. The build also writes `assemblr` in `manifest.jsonc`: the oldest
   Assemblr that reads the unit, which you commit with it:

   ```sh
   assemblr unit build
   assemblr unit check
   ```

5. **Publish it.** Set `version` in `manifest.jsonc`, such as `1.0.0`, then
   commit and publish. Publishing creates the version's Git tag:

   ```sh
   git add -A && git commit -m "First version"
   assemblr unit publish
   ```

   A published version never changes. To release a fix, raise the version
   and publish again.

To publish several units together, see
[Keep several units in one repository](catalogs.md). To let others use your
unit, see [Share your units](share.md).

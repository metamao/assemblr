# Assemblr

**Build projects from reusable, versioned units, and keep them up to date.**

Assemblr is a command-line tool that puts a project together from **units**.
A unit is a reusable part of a project. It can be a Dockerfile for Rust
services, a GitHub Actions or GitLab CI pipeline, or a Terraform module
layout. It can also be a Python package setup, Kubernetes manifests, an
Android build configuration, or your team's lint rules. Each unit lives in its own Git repository and has its own
versions.

**Assemblr works with any technology.** A unit is just files, so any
language, framework, cloud, or tool works. Assemblr never needs to know
what is inside.

You pick the units a project needs, and Assemblr writes their files into it.
When a unit publishes a new version, `assemblr update` brings the change in
and keeps the edits you made yourself. A project template is copied once and
then maintained alone. A unit keeps improving every project that uses it.

> **Status: private alpha.** The full workflow works today: create, share,
> compose, and update. File formats may still change between alpha releases.

## Why Assemblr

- **Fix once, update everywhere.** Bump a base image, patch a CI pipeline,
  or tighten a lint rule in one unit. Every project picks it up with
  `assemblr update`.
- **Your edits stay yours.** Assemblr merges a unit's new version with your
  own changes. When it cannot merge safely, it leaves conflict markers for
  you to resolve, as in a Git merge. It never overwrites your work silently.
- **Less AI slop.** A coding agent that adds a reviewed unit does not
  regenerate setup code from scratch, with new small mistakes each time. It
  spends its effort on the code that is unique to your project.
- **Golden paths without forks.** A unit can build on other units and amend
  their files, so a team can extend a shared base without copying it.
- **Start a new project in minutes** with the stack you already trust, and
  mix units from different authors.
- **Git is the only backend.** Units are Git repositories, versions are Git
  tags, and catalogs are Git repositories that list units. There is no
  server to run.
- **Safe and predictable.** Assemblr never runs code from a unit. It plans
  every change before it writes a file, published versions never change, and
  the same inputs always give the same result.
- **Built for coding agents.** `assemblr guide` teaches an agent the
  workflow, and the commands that report state offer `--json`.

## See it in action

Your team keeps the Dockerfile for its Rust services in one unit,
`@acme/rust-service`. The `orders-api` project uses it, and someone added an
`EXPOSE 8080` line to the project's Dockerfile by hand.

The unit then publishes version 1.1.0, which moves to Rust 1.90 and a smaller
distroless runtime image. In `orders-api`, one command brings that in:

```console
$ assemblr update
Updated: @acme/rust-service 1.0.0 -> 1.1.0.
note: updated assemblr-lock.json: @acme/rust-service 1.0.0 -> 1.1.0
Composed 2 units into 4 files: 1 merged.
  merged Dockerfile
```

Here is what changed in the Dockerfile. Both image updates arrived, and the
`EXPOSE 8080` line is still there:

```diff
-FROM rust:1.89-slim-bookworm AS build
+FROM rust:1.90-slim-bookworm AS build
 WORKDIR /src
 COPY . .
 RUN cargo build --release --locked

-FROM debian:bookworm-slim
+FROM gcr.io/distroless/cc-debian12
 COPY --from=build /src/target/release/orders-api /usr/local/bin/app
 EXPOSE 8080
 USER 65534
 CMD ["app"]
```

Every other service that uses the unit gets the same update the same way.

## How it works

| Concept | What it is |
| --- | --- |
| **Unit** | A Git repository with a `manifest.jsonc` and the files to deliver under `src/`. Its name looks like `@owner/name`. |
| **Version** | A published state of a unit, stored as a Git tag such as `v1.1.0`. It never changes once published. |
| **Project** | Any directory with an `assemblr.jsonc` that lists the units it uses and their options. |
| **Registry** | The list of units this machine knows, kept on this machine. |
| **Catalog** | A Git repository that lists units, so one command registers all of them. |
| **Amendment** | A unit's change to a file that one of the units it builds on delivers. |
| **Option** | A value a project sets for a unit's parameter, such as `name=orders-api`. |

**There is no hosted registry or package manager.** Assemblr only knows the
units in your local registry, and you decide what goes in it. Assemblr gives
you the tools to fill it and keep it current:

- `assemblr registry add <URL or path>` registers a unit's repository, or a
  catalog with all of its units.
- `assemblr registry sync` picks up the versions published since then.
- `assemblr search <words>` and `assemblr info <unit>` show what you can use.
- `assemblr catalog` lets a team publish one list of units that everyone
  registers with a single command.

Assemblr records what it delivered to a project in `assemblr-lock.json` and
`.assemblr/`. That record is how it tells your edits apart from a unit's
changes, so commit both with the project.

## Installation

You need [Git](https://git-scm.com/) 2.32 or newer. Assemblr runs on Linux,
WSL2, and Windows. It uses your existing Git credentials to read units in
private repositories and stores none of its own.

### Prebuilt binaries

Each release on [GitHub Releases](https://github.com/metamao/assemblr/releases)
carries prebuilt binaries for Linux x64 (WSL2 included) and Windows x64. If
no release is published yet, [build from source](#from-source).

You need the [GitHub CLI](https://cli.github.com/), signed in with
`gh auth login` to an account with access. Replace `v0.1.0-alpha.1` with the
release you want.

<details>
<summary><b>Linux and WSL2</b></summary>

```sh
tag=v0.1.0-alpha.1
dir=$(mktemp -d) && cd "$dir"
gh release download "$tag" --repo metamao/assemblr \
  --pattern 'assemblr-*-x86_64-unknown-linux-musl.tar.gz' --pattern SHA256SUMS
sha256sum --ignore-missing --check SHA256SUMS        # must print "...: OK"
tar -xzf assemblr-*.tar.gz
install -D -m 755 assemblr-*/assemblr ~/.local/bin/assemblr
assemblr --version                                    # ~/.local/bin must be on PATH
```

</details>

<details>
<summary><b>Windows (PowerShell)</b></summary>

```powershell
$tag = "v0.1.0-alpha.1"
$dir = New-Item -ItemType Directory (Join-Path $env:TEMP "assemblr-$tag") -Force
Set-Location $dir
gh release download $tag --repo metamao/assemblr --clobber `
  --pattern 'assemblr-*-x86_64-pc-windows-msvc.zip' --pattern SHA256SUMS
$zip = Get-Item assemblr-*.zip
$want = (Select-String -Path SHA256SUMS -Pattern ([regex]::Escape($zip.Name))).Line.Split(" ")[0]
if ((Get-FileHash $zip -Algorithm SHA256).Hash.ToLower() -ne $want) { throw "checksum mismatch" }
Expand-Archive $zip -DestinationPath . -Force
$bin = Join-Path $env:LOCALAPPDATA "Programs\assemblr"
New-Item -ItemType Directory $bin -Force | Out-Null
Copy-Item (Get-Item assemblr-*\assemblr.exe) $bin -Force
# once: put the directory on the user's PATH, then open a new terminal
[Environment]::SetEnvironmentVariable("Path", "$([Environment]::GetEnvironmentVariable('Path','User'));$bin", "User")
assemblr --version
```

</details>

The binaries are unsigned.

**Upgrade.** Run the same commands with the newer tag. They replace the binary
and nothing else. On Windows, close running `assemblr` processes first, and do
not add the `PATH` entry twice.

**Remove.** Delete the binary: `~/.local/bin/assemblr`, or
`%LOCALAPPDATA%\Programs\assemblr` and its `PATH` entry. Your projects are not
affected. To also forget the registered units, delete the registry directory:
`~/.local/share/assemblr` on Linux or `%LOCALAPPDATA%\assemblr` on Windows.

### From source

Use this when you want the latest development version, or when no release
is published yet. You need access to this private repository.

1. Install Rust with [rustup](https://rustup.rs/). The build uses the Rust
   version pinned in this repository, and rustup downloads it for you.
2. Clone the repository and install the binary:

   ```sh
   git clone https://github.com/metamao/assemblr.git
   cd assemblr
   cargo install --path . --locked
   ```

3. Check that it works:

   ```sh
   assemblr --version
   ```

   It prints the version and the commit it was built from, such as
   `assemblr 0.1.0-alpha.1 (e17c7acc88f2)`. If the command is not found, add
   `~/.cargo/bin` (or `%USERPROFILE%\.cargo\bin` on Windows) to your `PATH`.

To upgrade, run `git pull` in the clone and repeat step 2.

## Create a unit

The examples use the placeholder owner `acme`. Replace the names and URLs with
your own.

1. **Create it.** This makes a `rust-service/` directory with a manifest, a
   `src/` directory, a `README.md`, an `AGENTS.md`, and a Git repository:

   ```sh
   assemblr unit init @acme/rust-service
   cd rust-service
   ```

2. **Add your files** under `src/`. Each file is delivered to projects at the
   same path, so `src/Dockerfile` becomes the project's `Dockerfile`.

   To fill in a value per project, add the `.assemblr.tmpl` suffix, as in
   `src/Dockerfile.assemblr.tmpl`, and write `{{ name }}` where the value
   goes. Declare the parameter in `manifest.jsonc`:

   ```jsonc
   "params": {
     "name": { "type": "string", "description": "The binary name.", "example": "my-service" }
   }
   ```

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
   receive:

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

## Share your units

A unit is shared through its Git repository. Anyone who can read the
repository can use the unit, with the Git access they already have.

1. **Push the unit with its tags.** Create an empty repository on your Git
   host first, then connect it and push:

   ```sh
   git remote add origin https://github.com/acme/rust-service.git
   git push --follow-tags origin HEAD
   ```

   After each later `assemblr unit publish`, run the same `git push`. Push
   the units it builds on the same way.

2. **Hand out the URL.** Others register the unit on their machine:

   ```sh
   assemblr registry add https://github.com/acme/rust-service.git
   ```

   Registering a unit does not register the units it builds on. They must be
   registered too, which is what a catalog solves.

### Share many units with a catalog

A catalog is a Git repository that lists units. Anyone with access registers
all of them with one command, and picks up new versions with
`assemblr registry sync`.

1. **Create the catalog:**

   ```sh
   assemblr catalog init @acme/catalog
   cd catalog
   ```

2. **Add each unit** by its repository URL. Each command updates the catalog
   and commits:

   ```sh
   assemblr catalog add https://github.com/acme/rust-base.git
   assemblr catalog add https://github.com/acme/rust-service.git
   ```

3. **Describe and push it.** Fill in the `README.md` and `AGENTS.md` it
   created, and commit them. Create an empty repository on your Git host,
   then connect and push:

   ```sh
   git add -A && git commit -m "Describe the catalog"
   git remote add origin https://github.com/acme/catalog.git
   git push -u origin HEAD
   ```

**After a unit publishes a new version,** run `assemblr catalog refresh` in
the catalog, then `git push`. Everyone who registered the catalog then gets
the version with `assemblr registry sync`.

## Use units in a project

1. **Register the units** you want to use in your local registry. A catalog
   registers all of its units at once:

   ```sh
   assemblr registry add https://github.com/acme/catalog.git
   ```

   Then `assemblr search rust` and `assemblr info @acme/rust-service` show
   what is available and which options a unit needs.

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
   assemblr add @acme/rust-service --option name=orders-api
   ```

   If the unit needs a value you did not give, the error says which one and
   shows the command to run.

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

## Work with a coding agent

Assemblr is designed so that coding agents can use it as well as people can.

- `assemblr guide` prints a short guide for agents that matches the installed
  version.
- For Claude Code, run `assemblr skill install claude` once per machine. It
  installs a skill that tells Claude Code when to use Assemblr and to read
  the guide.
- For other agents, or a cloud session without the skill, add this line to
  the project's `AGENTS.md`: "Run `assemblr guide` before changing this
  project's setup, and follow it."

## Command overview

| Command | What it does |
| --- | --- |
| `assemblr unit` | Create, build, check, amend, update, and publish units |
| `assemblr catalog` | Create a catalog and add, remove, or refresh its units |
| `assemblr registry` | Register, list, sync, and remove units and catalogs |
| `assemblr search` / `info` | Find units and show their versions and parameters |
| `assemblr init` | Create a project's `assemblr.jsonc` |
| `assemblr add` / `remove` | Add or remove a unit and compose the project |
| `assemblr update` | Move units to the newest versions allowed |
| `assemblr status` | Show updates, edited files, and pending conflicts |
| `assemblr compose` | Compose the project again from `assemblr.jsonc` |
| `assemblr guide` / `skill` | Help coding agents use Assemblr |

Every command explains itself with `--help`, including examples.

## Tips

- **Formatters.** When Assemblr edits `assemblr.jsonc` or `manifest.jsonc`,
  it follows the file's style, trailing commas included, so a file Prettier
  formatted stays formatted. Assemblr alone writes `assemblr-lock.json` and
  `.assemblr/`, like `package-lock.json`: list them in `.prettierignore`.
- **Never edit `.assemblr/` or `assemblr-lock.json` by hand.**
  `assemblr status` reports what they record.

## Documentation

| Where | What |
| --- | --- |
| [`docs/domain/`](docs/domain/README.md) | What Assemblr means and promises: the product contracts |
| [`docs/domain/open-contracts.md`](docs/domain/open-contracts.md) | Product questions that are not decided yet |
| [`docs/adr/`](docs/adr/README.md) | Architectural decision records |
| [`AGENTS.md`](AGENTS.md) | How Assemblr is engineered: rules for contributors and coding agents |
| [`docs/user-facing-copy.md`](docs/user-facing-copy.md) | How Assemblr's help, messages, and README are written |

## Development

Assemblr is written in Rust. The toolchain is pinned in
`rust-toolchain.toml`, and rustup installs it automatically.

```sh
cargo run -- --help                                        # run the CLI
cargo sandbox -- assemblr --help                           # run it in an isolated registry
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo build --all-targets
```

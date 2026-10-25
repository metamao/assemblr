# Assemblr

**Build projects from reusable, versioned units, and keep them up to date.**

Assemblr is a command-line tool that puts a project together from **units**.
A unit is a reusable part of a project, such as a Node.js setup, a lint
configuration, or a Cloudflare deployment. Each unit lives in its own Git
repository and has its own versions.

You pick the units a project needs, and Assemblr writes their files into it.
When a unit publishes a new version, `assemblr update` brings the change in
and keeps the edits you made yourself. Unlike a project template, which you
copy once and then maintain alone, a unit keeps improving every project that
uses it.

> **Status: private alpha.** The full workflow works today: compose, update,
> author, publish, and share. File formats may still change between alpha
> releases.

## Highlights

- **Start from proven parts.** Add a unit in one command instead of copying
  setup code from an older project.
- **Updates that keep your changes.** Assemblr merges a unit's new version
  with your own edits. When both change the same lines, you get ordinary
  conflict markers to resolve, as in a Git merge.
- **Stack and customize.** A unit can depend on other units and change their
  files through **amendments**, so `@acme/web` can build on `@acme/node`.
- **Any technology.** Units are plain files. Assemblr does not care whether
  they hold TypeScript, Rust, Terraform, or CI workflows.
- **Git is the only backend.** Units are Git repositories, versions are Git
  tags, and a **catalog** shares a list of units. There is no server to run.
- **Safe and predictable.** Assemblr never runs code from a unit. It plans
  every change before it writes a file, published versions never change, and
  the same inputs always give the same result.
- **Built for coding agents.** `assemblr guide` teaches an agent the
  workflow, and commands that report state offer `--json`.

## See it in action

Two units, published by the same team:

- `@acme/node` delivers `package.json` and `.editorconfig`.
- `@acme/web` depends on `@acme/node`, adds `server.js`, and amends
  `package.json` to add a `start` script.

A project selects `@acme/web`, then adds its own `lint` script by hand. Later,
`@acme/node` 1.2.0 switches to ES modules. One command updates the project:

```console
$ assemblr update
Updated: @acme/node 1.1.0 -> 1.2.0.
note: updated assemblr-lock.json: @acme/node 1.1.0 -> 1.2.0
Composed 2 units into 3 files: 1 merged.
  merged package.json
```

The result combines all three sources:

```jsonc
{
  "name": "shop",
  "private": true,
  "type": "module",              // from the new @acme/node 1.2.0
  "scripts": {
    "start": "node server.js",   // from the @acme/web amendment
    "test": "node --test",
    "lint": "eslint ."           // your own edit, kept
  }
}
```

## How it works

| Concept | What it is |
| --- | --- |
| **Unit** | A Git repository with a `manifest.jsonc` and the files to deliver under `src/`. Its name looks like `@owner/name`. |
| **Version** | A published state of a unit, stored as a Git tag such as `v1.2.0`. It never changes once published. |
| **Project** | Any directory with an `assemblr.jsonc` that lists the units it uses and their options. |
| **Registry** | The list of units this machine knows. You add units to it with `assemblr registry add`. |
| **Catalog** | A Git repository that lists units, so one command registers all of them. |
| **Amendment** | A unit's change to a file that one of its dependencies delivers. |
| **Option** | A value a project sets for a unit's parameter, such as `appName=shop`. |

Assemblr records what it delivered in `assemblr-lock.json` and `.assemblr/`.
That record is how it tells your edits apart from a unit's changes. Commit
both with the project.

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

## Use units in a project

The examples use the placeholder owner `acme`. Replace the names and URLs with
your own.

1. **Register the units** you want to use. The easiest way is a catalog,
   which registers all of its units at once:

   ```sh
   assemblr registry add https://github.com/acme/catalog.git
   ```

   You can also register a single unit by its repository URL or a local path
   such as `../web`. Register the units it depends on as well: registering a
   unit does not register its dependencies. `assemblr search <words>` and
   `assemblr info @acme/web` then show what is available.

2. **Create the project.** In a new or existing directory, run:

   ```sh
   assemblr init
   ```

   It creates `assemblr.jsonc` and an `AGENTS.md` for coding agents.

3. **Add a unit.** Assemblr writes its files into the project:

   ```sh
   assemblr add @acme/web --option appName=shop
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
as in a Git merge. That happens when an update and your edits change the
same lines, or when a unit delivers a file you already had. Edit the file to
resolve the markers, then run `assemblr compose --continue`. To undo the
whole composition instead, run `assemblr compose --abort`.

## Create a unit

1. **Create it.** This makes a `web/` directory with a manifest, a `src/`
   directory, a `README.md`, an `AGENTS.md`, and a Git repository:

   ```sh
   assemblr unit init @acme/web
   cd web
   ```

2. **Add your files** under `src/`. Each file is delivered to projects at the
   same path. To insert an option's value, name the file with the
   `.assemblr.tmpl` suffix and write `{{ appName }}` where the value goes.
   Declare the parameter in `manifest.jsonc`:

   ```jsonc
   "params": {
     "appName": { "type": "string", "description": "The package name.", "example": "my-app" }
   }
   ```

3. **Build on other units** (optional). Add a dependency, then copy one of
   its files to change it:

   ```sh
   assemblr unit add @acme/node
   assemblr unit amend package.json
   ```

   Edit the copy under `amendments/`. Assemblr merges it with future versions
   of `@acme/node`.

4. **Build and check it.** `build/` shows exactly what a project would
   receive:

   ```sh
   assemblr unit build
   assemblr unit check
   ```

5. **Publish it.** Set `version` in `manifest.jsonc`, commit, then tag and
   push:

   ```sh
   git add -A && git commit -m "First version"
   assemblr unit publish
   git push --follow-tags origin HEAD
   ```

   Before the first push, create an empty repository on your Git host and
   connect it with `git remote add origin <URL>`.

   A published version never changes. To release a fix, raise the version
   and publish again.

## Share units with a catalog

A catalog is a Git repository that lists units. Anyone with access registers
all of them with one `assemblr registry add`.

1. **Create the catalog:**

   ```sh
   assemblr catalog init @acme/catalog
   cd catalog
   ```

2. **Add each unit** by its repository URL. Each command updates the catalog
   and commits:

   ```sh
   assemblr catalog add https://github.com/acme/node.git
   assemblr catalog add https://github.com/acme/web.git
   ```

3. **Describe and push it.** Fill in the `README.md` and `AGENTS.md` it
   created and commit them. Create an empty repository on your Git host, then
   connect and push:

   ```sh
   git add -A && git commit -m "Describe the catalog"
   git remote add origin https://github.com/acme/catalog.git
   git push -u origin HEAD
   ```

4. **Use it** on any machine:

   ```sh
   assemblr registry add https://github.com/acme/catalog.git
   ```

**After a unit publishes a new version,** run `assemblr catalog refresh` in
the catalog and `git push`. Other machines then pick up the version with
`assemblr registry sync`.

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
| `assemblr init` | Create a project's `assemblr.jsonc` |
| `assemblr add` / `remove` | Add or remove a unit and compose the project |
| `assemblr update` | Move units to the newest versions allowed |
| `assemblr status` | Show updates, edited files, and pending conflicts |
| `assemblr compose` | Compose the project again from `assemblr.jsonc` |
| `assemblr search` / `info` | Find units and show their versions and parameters |
| `assemblr registry` | Register, list, sync, and remove units and catalogs |
| `assemblr unit` | Create, build, check, amend, update, and publish units |
| `assemblr catalog` | Create a catalog and add, remove, or refresh its units |
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
`rust-toolchain.toml`, and rustup installs it automatically. Development
tracking lives in [issue #9](https://github.com/metamao/assemblr/issues/9).

```sh
cargo run -- --help                                        # run the CLI
cargo sandbox -- assemblr --help                           # run it in an isolated registry
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo build --all-targets
```

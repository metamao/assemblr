# Assemblr

Assemblr builds software projects from **units**: versioned, reusable parts
of a project that their authors keep in Git repositories, such as
`@metamao/cloudflare-worker`. A project selects units and Assemblr composes
their files; when a unit publishes a new version, Assemblr updates the
project without overwriting your own changes.

A unit can change a dependency's file through an **amendment**, an ordinary
copy of the file that Assemblr merges. Published versions are Git tags and
never change. Assemblr works with any technology and is built for people and
coding agents alike.

## Status

**Private alpha.** Assemblr composes, builds, publishes, and updates units
and consumer projects from the local registry. Persisted formats may still
change between alpha releases. The implementation is built in increments
tracked in GitHub [issue #9](https://github.com/metamao/assemblr/issues/9).

## Installation

Releases are prebuilt binaries for Windows x64 and Linux x64 (WSL2
included), attached to
[GitHub Releases](https://github.com/metamao/assemblr/releases) of this
private repository. You need the [GitHub CLI](https://cli.github.com/)
signed in to an account with access (`gh auth login`); Assemblr stores no
credentials. Replace `v0.1.0-alpha.1` with the release you want.

**Linux and WSL2**

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

**Windows (PowerShell)**

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

`assemblr --version` prints the version and the source commit of the build.

**Upgrade.** Repeat the same commands with the newer tag: they replace the
binary. Nothing else is touched: the registry (`~/.local/share/assemblr` on
Linux, `%LOCALAPPDATA%\assemblr` on Windows) and your projects'
`assemblr.jsonc`, `assemblr-lock.json`, and `.assemblr/` are not part of the
installation. On Windows, close running `assemblr` processes first. Do not
add the PATH entry twice.

**Remove.** Delete the binary (`~/.local/bin/assemblr`, or
`%LOCALAPPDATA%\Programs\assemblr` and its PATH entry). To forget all
registered units too, delete the registry directory above.

The binaries are unsigned and only the two targets exist. For development,
`cargo install --git https://github.com/metamao/assemblr --locked` builds the
current default branch from source (it needs Rust and Git access to the
repository).

## Usage

**Start a project from units.**

```sh
assemblr registry add ../web         # register a unit's Git repository
assemblr registry add git@github.com:acme/api.git   # or a remote one, by URL
assemblr init                        # create assemblr.jsonc and AGENTS.md here
assemblr add @acme/web               # select the unit and compose the project
assemblr add @acme/api --option port=8080   # set a parameter while adding
assemblr update                      # move to the newest versions allowed
```

`assemblr info @acme/web` shows a unit's versions and parameters. When an
update and your own edits change the same lines, or a unit delivers a file
you already had, the file is left with conflict markers: resolve them and run `assemblr compose --continue`, or undo
it with `assemblr compose --abort`.

**Create a unit.**

```sh
assemblr unit init @acme/web         # create web/ with a manifest, README, AGENTS.md, and a Git repository
cd web                               # add files under src/
assemblr unit add @acme/base         # declare a dependency and build into build/
assemblr unit amend package.json     # copy a dependency's file to change it
assemblr unit check                  # check it before publishing
git add -A && git commit -m "First version"
assemblr unit publish                # tag the committed version
git push --follow-tags               # share the commit and its tag
```

**Share units through a catalog.** A catalog is a Git repository that lists
unit repositories, so one `registry add` registers them all.

```sh
assemblr catalog init @acme/web      # create web/ with an empty registry.jsonc, README, and AGENTS.md
cd web
assemblr catalog add git@github.com:acme/node.git   # declare, index, and commit
git push
assemblr registry add git@github.com:acme/web.git   # on any machine with access
```

After a unit publishes a version and pushes its tag, run `assemblr catalog
refresh` in the catalog and `git push`; other machines then pick it up with
`assemblr registry sync`.

**Work with a coding agent.** `assemblr guide` prints a short guide for
coding agents that matches the installed version. For Claude Code, run
`assemblr skill install claude` once per machine: it installs a skill that
tells Claude Code when to use Assemblr and to read the guide. For another
agent, or a cloud session without the skill, add a line to the project's
`AGENTS.md`: "Run `assemblr guide` before changing this project's setup, and
follow it."

Every command explains itself with `--help`, including examples. The
concepts and the guarantees are in [`docs/domain/`](docs/domain/README.md).

## Documentation

| Where | What |
|-------|------|
| [`docs/domain/`](docs/domain/README.md) | What Assemblr means and promises: the canonical product contracts |
| [`docs/domain/open-contracts.md`](docs/domain/open-contracts.md) | Product questions that are not yet decided |
| [`AGENTS.md`](AGENTS.md) | How Assemblr is engineered: rules for contributors and coding agents |
| [`docs/user-facing-copy.md`](docs/user-facing-copy.md) | How Assemblr's help, messages, and README are written |
| [`docs/adr/`](docs/adr/README.md) | Architectural decision records |

## Development

The toolchain is pinned in `rust-toolchain.toml`, and rustup installs it
automatically.

```sh
cargo run -- --help
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo build --all-targets
```

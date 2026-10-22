# Assemblr

Assemblr is a Rust CLI for composing and maintaining opinionated, reusable
software **units**: versioned, author-owned implementations of software
capabilities and integrations, such as `@metamao/cloudflare-worker`.

Units are composed with their dependencies. They modify dependency files
through ordinary full-file copies (*amendments*), are validated by Assemblr,
and are published as immutable, versioned Git tags. Assemblr is
technology-agnostic and built for both humans and coding agents.

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

## Documentation

| Where | What |
|-------|------|
| [`docs/domain/`](docs/domain/README.md) | What Assemblr means and promises: the canonical product contracts |
| [`docs/domain/open-contracts.md`](docs/domain/open-contracts.md) | Product questions that are not yet decided |
| [`AGENTS.md`](AGENTS.md) | How Assemblr is engineered: rules for contributors and coding agents |
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

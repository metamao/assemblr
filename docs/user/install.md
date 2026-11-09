# Install Assemblr

You need [Git](https://git-scm.com/) 2.32 or newer. Assemblr runs on Linux,
WSL2, macOS, and Windows. It uses your existing Git credentials to read units in
private repositories and stores none of its own.

## Install script

Each script installs release `v0.1.0-alpha.3`. It downloads the release's
archive and `SHA256SUMS`, checks the archive against them, and installs the
binary for your user.

On Linux x64 (WSL2 included) and macOS on Apple Silicon, this installs to
`~/.local/bin`. It does not edit your `PATH`; it prints a hint when
`~/.local/bin` is not on it.

```sh
curl -fsSL https://raw.githubusercontent.com/metamao/assemblr/v0.1.0-alpha.3/install.sh | sh
```

On Windows x64, in PowerShell, this installs to
`%LOCALAPPDATA%\Programs\assemblr`. It does not edit your `PATH` either; it
prints the command that adds the directory.

```powershell
irm https://raw.githubusercontent.com/metamao/assemblr/v0.1.0-alpha.3/install.ps1 | iex
```

To read a script before running it, open
[`install.sh`](https://github.com/metamao/assemblr/blob/v0.1.0-alpha.3/install.sh)
or [`install.ps1`](https://github.com/metamao/assemblr/blob/v0.1.0-alpha.3/install.ps1).
Set `ASSEMBLR_VERSION` to install another release, such as `0.1.0-alpha.3`,
and `ASSEMBLR_INSTALL_DIR` to install somewhere else.

## Prebuilt binaries

Each release on [GitHub Releases](https://github.com/metamao/assemblr/releases)
carries prebuilt binaries for Linux x64 (WSL2 included), macOS on Apple
Silicon, and Windows x64. Intel Macs and other platforms
[build from source](#from-source). If no release is published yet, build from
source too.

Replace the tag with the release you want. The commands download
the archive and `SHA256SUMS`, check the archive against it, and install the
binary for your user.

<details>
<summary><b>Linux and WSL2</b></summary>

```sh
tag=v0.1.0-alpha.2
dir=$(mktemp -d) && cd "$dir"
url=https://github.com/metamao/assemblr/releases/download/$tag
curl -fsSLO "$url/assemblr-${tag#v}-x86_64-unknown-linux-musl.tar.gz"
curl -fsSLO "$url/SHA256SUMS"
sha256sum --ignore-missing --check SHA256SUMS        # must print "...: OK"
tar -xzf assemblr-*.tar.gz
install -D -m 755 assemblr-*/assemblr ~/.local/bin/assemblr
assemblr --version                                    # ~/.local/bin must be on PATH
```

</details>

<details>
<summary><b>macOS (Apple Silicon)</b></summary>

```sh
tag=v0.1.0-alpha.3
dir=$(mktemp -d) && cd "$dir"
url=https://github.com/metamao/assemblr/releases/download/$tag
curl -fsSLO "$url/assemblr-${tag#v}-aarch64-apple-darwin.tar.gz"
curl -fsSLO "$url/SHA256SUMS"
grep aarch64-apple-darwin SHA256SUMS | shasum -a 256 --check   # must print "...: OK"
tar -xzf assemblr-*.tar.gz
mkdir -p ~/.local/bin && rm -f ~/.local/bin/assemblr   # replace, never overwrite
cp assemblr-*/assemblr ~/.local/bin/
assemblr --version                                    # ~/.local/bin must be on PATH
```

</details>

<details>
<summary><b>Windows (PowerShell)</b></summary>

```powershell
$tag = "v0.1.0-alpha.2"
$dir = New-Item -ItemType Directory (Join-Path $env:TEMP "assemblr-$tag") -Force
Set-Location $dir
$ProgressPreference = "SilentlyContinue"    # the progress bar slows downloads a lot
$url = "https://github.com/metamao/assemblr/releases/download/$tag"
$zip = "assemblr-$($tag.Substring(1))-x86_64-pc-windows-msvc.zip"
Invoke-WebRequest "$url/$zip" -OutFile $zip -UseBasicParsing
Invoke-WebRequest "$url/SHA256SUMS" -OutFile SHA256SUMS -UseBasicParsing
$want = (Select-String -Path SHA256SUMS -Pattern ([regex]::Escape($zip))).Line.Split(" ")[0]
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

The binaries are unsigned. On macOS, `curl` does not mark its downloads as
quarantined, so Gatekeeper does not stop the binary; an archive downloaded
with a browser is marked and may be stopped.

**Upgrade.** Run the newer release's install script, or the same commands
with the newer tag. They replace the binary
and nothing else. On Windows, close running `assemblr` processes first, and do
not add the `PATH` entry twice.

**Remove.** Delete the binary: `~/.local/bin/assemblr`, or
`%LOCALAPPDATA%\Programs\assemblr` and its `PATH` entry. Your projects are not
affected. To also forget the registered units, delete the registry directory:
`~/.local/share/assemblr` on Linux, `~/Library/Application Support/assemblr`
on macOS, or `%LOCALAPPDATA%\assemblr` on Windows.

## From source

Use this when you want the latest development version, or when no release
is published yet.

1. Install Rust with [rustup](https://rustup.rs/). The build uses the Rust
   version pinned in the repository, and rustup downloads it for you.
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
   `assemblr 0.1.0-alpha.2 (e17c7acc88f2)`. If the command is not found, add
   `~/.cargo/bin` (or `%USERPROFILE%\.cargo\bin` on Windows) to your `PATH`.

To upgrade, run `git pull` in the clone and repeat step 2.

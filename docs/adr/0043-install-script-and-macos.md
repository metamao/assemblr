# 0043: Install scripts for Linux, macOS, and Windows, and a macOS build for Apple Silicon

- **Status:** Accepted
- **Date:** 2026-11-08

## Context

The repository is public, and the website will offer one-line installs,
`curl -fsSL <site>/install.sh | sh` and `irm <site>/install.ps1 | iex`.
ADR 0026 chose documented commands over a script for a two-platform private
alpha, and left macOS for "a new matrix entry". Nobody on the project can run a Mac, so any macOS support
must be checked by CI.

What was checked before deciding:

- The code has no Linux-only paths: platform code is `cfg(unix)` or
  `cfg(windows)`, the registry uses `dirs::data_local_dir()`
  (`~/Library/Application Support` on macOS), and path collisions already
  account for case-insensitive and normalizing file systems.
  `cargo clippy --all-targets --target aarch64-apple-darwin` passes. The
  tests have never run on macOS.
- Every release is a prerelease, so GitHub's
  `releases/latest` answers 404 for this repository. A script cannot ask
  GitHub for "the latest release" without listing every release.
- `curl` does not mark downloads as quarantined, so Gatekeeper does not stop
  an unsigned binary it fetched. On Apple Silicon every binary must carry a
  signature; the Apple linker adds an ad hoc one, so running the binary in
  CI proves it.

## Decision

1. **One more target, `aarch64-apple-darwin`**, built and smoke-tested on
   `macos-latest`, packaged as `.tar.gz` like Linux. No Intel build: GitHub's
   last Intel runner image is due to retire in 2027, Apple sells no Intel
   Macs, and Intel Macs can build from source. Adding one later is a matrix
   entry.
2. **The tests run on macOS** where they run on Windows: in the release
   workflow's validation, and in CI after a merge to `main` or when started
   by hand. Pull requests stay Linux-only. Runners are free for public
   repositories.
3. **`install.sh` at the repository root** installs on Linux x64 and macOS
   on Apple Silicon. It runs the documented steps: it downloads the archive
   and `SHA256SUMS`, checks the archive, and renames the binary into
   `~/.local/bin`. It edits no shell profile and no `PATH`. It needs `curl`,
   `tar`, and `sha256sum` or `shasum`, and runs under any POSIX `sh`.
   **`install.ps1`** does the same on Windows x64, into
   `%LOCALAPPDATA%\Programs\assemblr`, and runs in Windows PowerShell 5.1
   and PowerShell 7. Like `install.sh`, it does not edit the `PATH`; it
   prints the command that adds the directory. On the owner's machine, Norton's
   behavioral protection stopped an earlier version at the point where it
   wrote the user `PATH` to the registry, quarantined the unsigned
   `assemblr.exe`, and left the `PATH` empty. An unknown, unsigned program
   that changes the `PATH` looks like malware. The script throws instead of
   calling `exit`, which would close a window that runs it through `iex`.
4. **A script installs the release it belongs to.** Its default version
   equals the package version, and a test keeps them equal, so the script
   at tag `vX` installs `vX`. `ASSEMBLR_VERSION` picks another
   one. The website serves the script from the tag it documents, so what
   it serves and what it installs stay consistent, and nothing depends on
   GitHub's API.
5. **The release workflow tests the scripts** on each platform: it installs
   the archive it just built through `install.sh` or, in Windows PowerShell,
   `install.ps1`, from a `file://` copy of the release
   (`ASSEMBLR_DOWNLOAD_URL`), and checks the version.

## Alternatives considered

- *Find the newest release through the GitHub API.* Needs a listing that
  includes prereleases, depends on an unauthenticated rate limit, and makes
  the same command install different versions over time.
- *An Intel build, or a universal binary.* Doubles the macOS packaging, and
  the Intel half could not be run in CI once the Intel runner retires.
- *Attaching the script to each release.* Raw files at a tag are just as
  immutable, since tags are never moved, and need no release change.

## Consequences

- Upgrading is running the newer tag's script. `install.sh` replaces the
  binary in one rename, even while an older one runs; `install.ps1` cannot
  replace a running `assemblr.exe` and asks to close it.
- A version bump also bumps the default version in both scripts; the test
  fails otherwise.
- The checksum proves integrity against the release, not authenticity, as
  in ADR 0026. The macOS binary is not notarized.
- macOS correctness rests on CI, not on a person using a Mac.
- The binaries have no publisher reputation with antivirus software. Code
  signing would give them one; it needs a certificate and is left for a later
  decision.

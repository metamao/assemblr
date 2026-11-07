# 0026: Private alpha distribution: prebuilt binaries as draft GitHub Releases

- **Status:** Accepted
- **Date:** 2026-10-22

## Context

The owner wants to use Assemblr from their own machines, independently of
this repository, on Windows x64 and Linux x64 (WSL2 included). The
repository is private. `cargo install --git` needs a Rust toolchain, a build
of every dependency, and Git credentials for Cargo; a prebuilt binary needs
only the GitHub CLI the owner already uses. No public distribution, signing,
or self-update is required (#68, D-25).

## Decision

1. **Prebuilt binaries are the primary install; `cargo install --git` is a
   developer fallback.** Binaries are attached to GitHub Releases of the
   private repository. Downloading uses `gh release download`, which reuses
   `gh auth`; Assemblr stores no credentials and has no network code.
   *Amended (2026-11-07):* the repository is public, so the documented
   install downloads the release assets over HTTPS with `curl` or
   `Invoke-WebRequest` and needs neither `gh` nor an account. Releases are
   still created as draft prereleases (point 4), and the commands work once
   the owner publishes one.
2. **Targets.** `x86_64-pc-windows-msvc` and `x86_64-unknown-linux-musl`. The
   Linux binary is static, so it does not depend on the glibc of the build
   runner and runs on any x64 Linux, WSL2 included. The code has no C
   dependencies, so musl costs nothing.
3. **Versions are SemVer prereleases** (`0.1.0-alpha.1`), tags are
   `v<version>`, and the workflow refuses a tag that differs from
   `Cargo.toml`. `assemblr --version` prints `assemblr <version> (<commit>)`;
   `build.rs` takes the commit from `ASSEMBLR_COMMIT` (set by the release
   build to the exact commit) or from `git rev-parse`, and omits it when
   neither exists.
4. **One hand-started workflow, `release.yml`.** It validates (fmt, clippy,
   tests on Linux and Windows) before packaging, builds with `--locked`,
   packages `assemblr-<version>-<target>.tar.gz` (Linux) or `.zip` (Windows)
   holding one directory with the binary, smoke-tests each archive outside the
   checkout, and creates a **draft prerelease** with `SHA256SUMS`, targeting
   the commit that was built (`github.sha`, which every job checks out, so
   a later push to `main` cannot change it). The workflow refuses a tag that
   already exists and verifies that the draft stores that commit. Nothing is
   released on merges. The owner
   publishes the draft, and publishing creates the tag at that commit; it is
   never moved. Started by hand on another branch, for example a pull
   request that touches the packaging, it runs the same jobs without the
   release step. Pull requests do not start it automatically, to save
   Actions minutes.
5. **Install, upgrade, and removal are documented commands**, not scripts: an
   upgrade replaces the binary. User state (the registry in the platform's
   local data directory, and every consumer project) is never part of the
   installation, so an upgrade cannot touch it.

## Alternatives considered

- *`cargo install --git` as the primary path.* Needs Rust, minutes of
  compilation, and Cargo-side Git credentials for a private repository.
- *A glibc Linux binary.* Tied to the runner's glibc; older WSL distributions
  could fail to start it.
- *An install script, `assemblr self-update`, `cargo-binstall`, or a package
  manager.* More surface than a two-platform private alpha needs; each can be
  added when a requirement appears.
- *The workflow creating the tag and a published release.* Removes the
  owner's last look at the artifacts and checksums.

## Consequences

- The binaries are unsigned. Windows SmartScreen may warn on a browser
  download; files fetched with `gh` carry no mark of the web, so it normally
  does not. The checksum file proves integrity against the release, not
  authenticity. *Amended (2026-11-07):* the documented install no longer
  uses `gh`; whether files that `Invoke-WebRequest` saves carry the mark of
  the web is not verified.
- Only the two targets exist. macOS and arm64 need a new matrix entry.
- A release is reproducible from its tag: `--locked`, the pinned toolchain,
  and the commit are all fixed.
- The persisted formats are not versioned by the release; compatibility
  between alpha builds is not promised. *Amended (ADR 0041):* from the first
  beta, later versions read every format a beta wrote or published.

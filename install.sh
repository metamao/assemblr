#!/bin/sh
# Installs the Assemblr binary from GitHub Releases, on Linux x64 (WSL2
# included) and macOS on Apple Silicon (docs/user/install.md, docs/adr/0043).
#
#   curl -fsSL https://raw.githubusercontent.com/metamao/assemblr/<tag>/install.sh | sh
#
# It downloads the archive and SHA256SUMS of one release, checks the archive
# against them, and puts the binary in a directory you own. It changes
# nothing else: no shell profile, no PATH, and no registry.
#
# Environment:
#   ASSEMBLR_VERSION      the release to install, such as 0.1.0-alpha.2
#                         (default: the release this script belongs to)
#   ASSEMBLR_INSTALL_DIR  where to put the binary (default: ~/.local/bin)
#   ASSEMBLR_DOWNLOAD_URL where releases are downloaded from (default:
#                         GitHub Releases); a file:// URL tests local archives

set -eu

# The release this script belongs to. A test keeps it equal to Cargo.toml.
default_version=0.1.0-alpha.3
releases=https://github.com/metamao/assemblr/releases/download
from_source=https://github.com/metamao/assemblr/blob/main/docs/user/install.md#from-source

fail() {
    printf 'error: %s\n' "$1" >&2
    shift
    for line in "$@"; do
        printf '%s\n' "$line" >&2
    done
    exit 1
}

need() {
    command -v "$1" > /dev/null 2>&1 ||
        fail "$1 is not installed" "hint: install $1 and run this script again"
}

target() {
    os=$(uname -s)
    arch=$(uname -m)
    # A shell running under Rosetta reports x86_64 on Apple Silicon.
    if [ "$os/$arch" = Darwin/x86_64 ] &&
        [ "$(sysctl -n sysctl.proc_translated 2> /dev/null || true)" = 1 ]; then
        arch=arm64
    fi
    case "$os/$arch" in
        Linux/x86_64 | Linux/amd64) echo x86_64-unknown-linux-musl ;;
        Darwin/arm64 | Darwin/aarch64) echo aarch64-apple-darwin ;;
        MINGW* | MSYS* | CYGWIN*)
            fail "this script does not install on Windows" \
                "hint: follow the Windows steps in $from_source" ;;
        *)
            fail "no prebuilt binary for $os $arch" \
                "hint: build Assemblr from source: $from_source" ;;
    esac
}

sha256() {
    if command -v sha256sum > /dev/null 2>&1; then
        sha256sum "$1"
    elif command -v shasum > /dev/null 2>&1; then
        shasum -a 256 "$1"
    else
        fail "neither sha256sum nor shasum is installed" \
            "note: the download cannot be checked, so nothing was installed"
    fi
}

main() {
    need curl
    need tar
    need uname

    version=${ASSEMBLR_VERSION:-$default_version}
    version=${version#v}
    install_dir=${ASSEMBLR_INSTALL_DIR:-${HOME:?}/.local/bin}
    # Relative to where the script started, not to the download directory.
    case $install_dir in
        /*) ;;
        *) install_dir=$PWD/$install_dir ;;
    esac
    base=${ASSEMBLR_DOWNLOAD_URL:-$releases}
    target=$(target)
    name=assemblr-$version-$target
    archive=$name.tar.gz
    url=$base/v$version

    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    cd "$tmp"

    printf 'Downloading Assemblr %s for %s\n' "$version" "$target"
    curl -fsSLO "$url/SHA256SUMS" ||
        fail "could not download $url/SHA256SUMS" \
            "hint: check that release v$version exists: https://github.com/metamao/assemblr/releases"
    curl -fsSLO "$url/$archive" ||
        fail "release v$version has no $archive" \
            "hint: pick a release that lists it, or build from source: $from_source"

    want=$(awk -v f="$archive" '$2 == f || $2 == "*" f { print $1 }' SHA256SUMS)
    # Not in a pipeline, so a failure in sha256 stops the script.
    have=$(sha256 "$archive")
    have=${have%% *}
    [ -n "$want" ] ||
        fail "SHA256SUMS of v$version does not list $archive" "note: nothing was installed"
    [ "$want" = "$have" ] ||
        fail "$archive does not match its checksum in SHA256SUMS" "note: nothing was installed"

    tar -xzf "$archive"
    [ -f "$name/assemblr" ] || fail "$archive has no $name/assemblr" "note: nothing was installed"

    # Copy beside the target, then rename over it: the rename replaces the
    # binary in one step, even while an older assemblr is running.
    mkdir -p "$install_dir"
    cp "$name/assemblr" "$install_dir/.assemblr.$$"
    chmod 755 "$install_dir/.assemblr.$$"
    mv -f "$install_dir/.assemblr.$$" "$install_dir/assemblr"

    printf 'Installed %s\n' "$("$install_dir/assemblr" --version)"
    printf '  binary: %s\n' "$install_dir/assemblr"
    case ":$PATH:" in
        *":$install_dir:"*) ;;
        *) printf 'hint: add %s to your PATH\n' "$install_dir" ;;
    esac
}

# Everything runs inside main, so a download cut short runs nothing.
main "$@"

# Installs the Assemblr binary from GitHub Releases on Windows x64
# (docs/user/install.md, docs/adr/0043).
#
#   irm https://raw.githubusercontent.com/metamao/assemblr/<tag>/install.ps1 | iex
#
# It downloads the archive and SHA256SUMS of one release, checks the archive
# against them, and puts assemblr.exe in a directory you own. It changes
# nothing else: no PATH, no registry, and no Assemblr registry.
#
# Environment:
#   ASSEMBLR_VERSION      the release to install, such as 0.1.0-alpha.2
#                         (default: the release this script belongs to)
#   ASSEMBLR_INSTALL_DIR  where to put the binary
#                         (default: %LOCALAPPDATA%\Programs\assemblr)
#   ASSEMBLR_DOWNLOAD_URL where releases are downloaded from (default:
#                         GitHub Releases); a file:// URL tests local archives

# A script block keeps the variables out of the caller's session, and errors
# are thrown rather than calling exit, which would close a window run by iex.
& {
    $ErrorActionPreference = "Stop"
    $ProgressPreference = "SilentlyContinue"    # the progress bar slows downloads a lot

    # The release this script belongs to. A test keeps it equal to Cargo.toml.
    $defaultVersion = "0.1.0-alpha.3"
    $releases = "https://github.com/metamao/assemblr/releases/download"
    $fromSource = "https://github.com/metamao/assemblr/blob/main/docs/user/install.md#from-source"
    $target = "x86_64-pc-windows-msvc"

    function Fail([string[]] $lines) {
        throw ("error: " + ($lines -join "`n"))
    }

    # Windows PowerShell leaves $IsWindows unset; PowerShell 7 sets it.
    if ($IsWindows -eq $false) {
        Fail "this script installs on Windows only", "hint: on Linux and macOS, use install.sh: $fromSource"
    }
    $arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
    if ($arch -ne "AMD64") {
        Fail "no prebuilt binary for Windows $arch", "hint: build Assemblr from source: $fromSource"
    }

    $version = if ($env:ASSEMBLR_VERSION) { $env:ASSEMBLR_VERSION } else { $defaultVersion }
    $version = $version -replace "^v", ""
    $installDir = if ($env:ASSEMBLR_INSTALL_DIR) { $env:ASSEMBLR_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "Programs\assemblr" }
    # Relative to where the script started, not to the download directory.
    $installDir = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($installDir)
    $base = if ($env:ASSEMBLR_DOWNLOAD_URL) { $env:ASSEMBLR_DOWNLOAD_URL } else { $releases }
    $name = "assemblr-$version-$target"
    $archive = "$name.zip"
    $url = "$base/v$version"

    # Windows PowerShell 5.1 may not offer TLS 1.2, which GitHub requires.
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    function Get-Release([string] $file, [string[]] $failure) {
        $source = "$url/$file"
        try {
            if ($source.StartsWith("file://")) {
                Copy-Item ([Uri] $source).LocalPath $file
            } else {
                Invoke-WebRequest $source -OutFile $file -UseBasicParsing
            }
        } catch {
            Fail ($failure + "note: $($_.Exception.Message)")
        }
    }

    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("assemblr-" + [Guid]::NewGuid())
    New-Item -ItemType Directory $tmp | Out-Null
    Push-Location $tmp
    try {
        Write-Host "Downloading Assemblr $version for $target"
        Get-Release "SHA256SUMS" @("could not download $url/SHA256SUMS", "hint: check that release v$version exists: https://github.com/metamao/assemblr/releases")
        Get-Release $archive @("release v$version has no $archive", "hint: pick a release that lists it, or build from source: $fromSource")

        $want = $null
        foreach ($line in Get-Content SHA256SUMS) {
            $fields = $line -split "\s+", 2
            if ($fields.Count -eq 2 -and $fields[1].TrimStart("*") -eq $archive) { $want = $fields[0] }
        }
        if (-not $want) {
            Fail "SHA256SUMS of v$version does not list $archive", "note: nothing was installed"
        }
        if ((Get-FileHash $archive -Algorithm SHA256).Hash -ne $want) {
            Fail "$archive does not match its checksum in SHA256SUMS", "note: nothing was installed"
        }

        Expand-Archive $archive -DestinationPath . -Force
        $exe = Join-Path $name "assemblr.exe"
        if (-not (Test-Path $exe -PathType Leaf)) {
            Fail "$archive has no $exe", "note: nothing was installed"
        }

        New-Item -ItemType Directory $installDir -Force | Out-Null
        $installed = Join-Path $installDir "assemblr.exe"
        try {
            Copy-Item $exe $installed -Force
        } catch {
            Fail "could not write $installed", "note: $($_.Exception.Message)", "hint: if assemblr is running, close it and run this script again"
        }
    } finally {
        Pop-Location
        Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }

    Write-Host "Installed $(& $installed --version)"
    Write-Host "  binary: $installed"

    $onPath = @($env:Path -split ";" | Where-Object { $_.TrimEnd("\") -eq $installDir.TrimEnd("\") }).Count -gt 0
    if (-not $onPath) {
        Write-Host "hint: add $installDir to your user PATH, then open a new terminal:"
        Write-Host "  [Environment]::SetEnvironmentVariable(`"Path`", [Environment]::GetEnvironmentVariable(`"Path`", `"User`") + `";$installDir`", `"User`")"
    }
}

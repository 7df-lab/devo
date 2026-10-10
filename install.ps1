# install.ps1 — Download and install the latest devo binary for Windows.
#
# Usage (run as administrator is not required, installs to user-local bin):
#   irm https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1 | iex
#
# Pin a specific version:
#   $env:VERSION = "v0.2.0"; irm https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1 | iex
#
#
# Offline install from assets next to install.ps1:
#   .\install.ps1 -Offline

param(
    [string]$Version = $env:VERSION,
    [switch]$Offline
)

$ErrorActionPreference = "Stop"
$Repo = "7df-lab/devo"
$RipgrepRepo = "BurntSushi/ripgrep"

# ── Platform detection ───────────────────────────────────────────────────
function Get-Target {
    $arch = if ($env:PROCESSOR_ARCHITEW6432 -eq "ARM64" -or $env:PROCESSOR_ARCHITECTURE -eq "ARM64") { "aarch64" } elseif ([Environment]::Is64BitOperatingSystem) { "x86_64" } else {
        Write-Error "32-bit Windows is not supported"
        exit 1
    }
    return "${arch}-pc-windows-msvc"
}

function Get-RipgrepTarget {
    $arch = if ($env:PROCESSOR_ARCHITEW6432 -eq "ARM64" -or $env:PROCESSOR_ARCHITECTURE -eq "ARM64") { "aarch64" } elseif ([Environment]::Is64BitOperatingSystem) { "x86_64" } else {
        Write-Error "32-bit Windows is not supported for ripgrep"
        exit 1
    }
    return "${arch}-pc-windows-msvc"
}

function Normalize-PathEntry {
    param(
        [string]$Value
    )

    $normalized = $Value.Trim()
    while ($normalized.Length -gt 3 -and $normalized.EndsWith("\\")) {
        $normalized = $normalized.Substring(0, $normalized.Length - 1)
    }

    return $normalized
}

function Test-PathEntryPresent {
    param(
        [string]$PathValue,
        [string]$Entry
    )

    if ([string]::IsNullOrWhiteSpace($PathValue)) {
        return $false
    }

    $normalizedEntry = Normalize-PathEntry $Entry
    foreach ($candidate in ($PathValue -split ";")) {
        if ([string]::IsNullOrWhiteSpace($candidate)) {
            continue
        }

        if ((Normalize-PathEntry $candidate) -ieq $normalizedEntry) {
            return $true
        }
    }

    return $false
}

function Add-InstallDirToPath {
    param(
        [string]$InstallDir
    )

    $currentUserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if (-not (Test-PathEntryPresent -PathValue $currentUserPath -Entry $InstallDir)) {
        $newUserPath = if ([string]::IsNullOrWhiteSpace($currentUserPath)) {
            $InstallDir
        } else {
            "$InstallDir;$currentUserPath"
        }
        [Environment]::SetEnvironmentVariable("Path", $newUserPath, "User")
    }

    if (-not (Test-PathEntryPresent -PathValue $env:Path -Entry $InstallDir)) {
        $env:Path = if ([string]::IsNullOrWhiteSpace($env:Path)) {
            $InstallDir
        } else {
            "$InstallDir;$env:Path"
        }
    }
}

function Broadcast-EnvironmentChange {
    if (-not ("Win32.NativeMethods" -as [type])) {
        Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;

namespace Win32 {
    public static class NativeMethods {
        [DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
        public static extern IntPtr SendMessageTimeout(
            IntPtr hWnd,
            int Msg,
            UIntPtr wParam,
            string lParam,
            int fuFlags,
            int uTimeout,
            out UIntPtr lpdwResult);
    }
}
"@
    }

    $result = [UIntPtr]::Zero
    [Win32.NativeMethods]::SendMessageTimeout(
        [IntPtr]0xffff,
        0x1A,
        [UIntPtr]::Zero,
        "Environment",
        2,
        5000,
        [ref]$result
    ) | Out-Null
}

# ── Resolve version ──────────────────────────────────────────────────────
function Resolve-GitHubLatestVersion {
    param(
        [string]$RepoName
    )

    $latest = Invoke-RestMethod -Uri "https://api.github.com/repos/$RepoName/releases/latest"
    return $latest.tag_name
}

function Resolve-Version {
    if ($Version) {
        return Normalize-Version $Version
    }

    return Normalize-Version (Resolve-GitHubLatestVersion -RepoName $Repo)
}

function Normalize-Version {
    param(
        [string]$Value
    )

    $normalized = $Value.Trim()
    if ($normalized.StartsWith("v")) {
        return $normalized
    }

    return "v$normalized"
}

function Test-Truthy {
    param(
        [string]$Value
    )

    return $Value -match "^(1|true|yes|on)$"
}



function Get-DevoHome {
    if (-not [string]::IsNullOrWhiteSpace($env:DEVO_HOME)) {
        return $env:DEVO_HOME
    }

    return Join-Path $HOME ".devo"
}

function Normalize-DevoVersionOutput {
    param(
        [string]$RawVersion
    )

    foreach ($part in ($RawVersion -split "\s+")) {
        if ($part -match "^v\d+\.\d+\.\d+.*$") {
            return $part
        }
        if ($part -match "^\d+\.\d+\.\d+.*$") {
            return "v$part"
        }
    }

    if (-not [string]::IsNullOrWhiteSpace($RawVersion)) {
        return $RawVersion.Trim()
    }

    return "unknown"
}

function Get-ExistingDevoPath {
    param(
        [string]$InstallDir
    )

    $installedTarget = Join-Path $InstallDir "devo.exe"
    if (Test-Path $installedTarget) {
        return $installedTarget
    }

    $command = Get-Command "devo.exe" -ErrorAction SilentlyContinue
    if ($command) {
        if ($command.Source) {
            return $command.Source
        }
        return $command.Path
    }

    $command = Get-Command "devo" -ErrorAction SilentlyContinue
    if ($command) {
        if ($command.Source) {
            return $command.Source
        }
        return $command.Path
    }

    return $null
}

function Get-InstalledDevoVersion {
    param(
        [string]$DevoPath
    )

    try {
        $rawVersion = (& $DevoPath --version 2>$null) -join " "
    } catch {
        $rawVersion = ""
    }

    return Normalize-DevoVersionOutput $rawVersion
}

function Write-VersionTransition {
    param(
        [string]$InstallDir,
        [string]$TargetVersion
    )

    $installedPath = Get-ExistingDevoPath -InstallDir $InstallDir
    if ($installedPath) {
        $currentVersion = Get-InstalledDevoVersion -DevoPath $installedPath
    } else {
        $currentVersion = "not installed"
    }

    Write-Host "Version: $currentVersion -> $TargetVersion"
}

function Test-DevoVersionInstalled {
    param(
        [string]$InstallDir,
        [string]$ExpectedVersion
    )

    $installedPath = Get-ExistingDevoPath -InstallDir $InstallDir
    if (-not $installedPath) {
        return $false
    }

    $installedVersion = Get-InstalledDevoVersion -DevoPath $installedPath
    if ($installedVersion -eq $ExpectedVersion -and (Test-DevoBundle -Directory $InstallDir)) {
        Write-Host "devo $ExpectedVersion is already installed at $installedPath"
        return $true
    }

    Write-Host "Found existing devo at $installedPath ($installedVersion)"
    return $false
}

# ── Banner ───────────────────────────────────────────────────────────────
function Print-Banner {
    Write-Host "Devo installer"
}

function Install-RipgrepSidecar {
    param(
        [string]$InstallDir,
        [string]$TempRoot
    )

    if ($env:DEVO_SKIP_RG_INSTALL -eq "1") {
        Write-Host "Skipping ripgrep sidecar install because DEVO_SKIP_RG_INSTALL=1."
        return
    }

    $targetPath = Join-Path $InstallDir "rg.exe"
    if (Test-Path $targetPath) {
        Write-Host "ripgrep sidecar is already installed at $targetPath"
        return
    }

    $rgTarget = Get-RipgrepTarget
    $rgVersion = Resolve-GitHubLatestVersion -RepoName $RipgrepRepo
    $rgArchiveUrl = "https://github.com/$RipgrepRepo/releases/download/$rgVersion/ripgrep-${rgVersion}-${rgTarget}.zip"
    $rgTmpDir = Join-Path $TempRoot "ripgrep"
    New-Item -ItemType Directory -Force -Path $rgTmpDir | Out-Null

    Write-Host "Downloading ripgrep $rgVersion for $rgTarget ..."

    $rgZipPath = Join-Path $rgTmpDir "ripgrep.zip"
    Invoke-WebRequest -Uri $rgArchiveUrl -OutFile $rgZipPath
    Expand-Archive -Path $rgZipPath -DestinationPath $rgTmpDir -Force

    $rgExe = Get-ChildItem -Recurse -Filter "rg.exe" -Path $rgTmpDir | Select-Object -First 1
    if (-not $rgExe) {
        Write-Error "rg.exe not found in the ripgrep archive"
    }

    Copy-Item -Path $rgExe.FullName -Destination $targetPath -Force
}



function Get-InstallerAssetDir {
    if (-not [string]::IsNullOrWhiteSpace($PSScriptRoot)) {
        return $PSScriptRoot
    }

    return (Get-Location).Path
}

function Get-FirstMatchingFile {
    param(
        [string]$Directory,
        [string]$Pattern
    )

    return Get-ChildItem -Path $Directory -Filter $Pattern -File |
        Sort-Object -Property Name |
        Select-Object -First 1
}

function Test-DevoBundle {
    param([string]$Directory)
    foreach ($name in @("runtime\manifest.json", "runtime\node\node.exe", "runtime\python\python.exe", "runtime\python-site\dill\__init__.py", "runtime\python-site\rlm\repl.py", "tui\src\index.js", "rg.exe", "devo.exe")) {
        if (-not (Test-Path -LiteralPath (Join-Path $Directory $name) -PathType Leaf)) { return $false }
    }
    try { return (Get-Content -LiteralPath (Join-Path $Directory "runtime\manifest.json") -Raw | ConvertFrom-Json).schema -eq 1 } catch { return $false }
}

function Test-ArchiveChecksum {
    param([string]$Archive, [string]$ChecksumFile, [string]$AssetName)
    $line = Get-Content -LiteralPath $ChecksumFile | Where-Object { $_ -match ("^[a-fA-F0-9]{64}  " + [regex]::Escape($AssetName) + "$") } | Select-Object -First 1
    if (-not $line -or (Get-FileHash -LiteralPath $Archive -Algorithm SHA256).Hash -ine $line.Substring(0, 64)) {
        throw "Release archive SHA-256 verification failed: $AssetName"
    }
}

function Install-DevoBundle {
    param([string]$Source, [string]$InstallDir)
    if (-not (Test-DevoBundle -Directory $Source)) {
        throw "Incomplete Devo archive. Download the complete runtime bundle; a standalone devo.exe cannot launch the TUI."
    }
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    $stage = Join-Path $InstallDir (".devo-install-" + [guid]::NewGuid().ToString("N"))
    $fresh = Join-Path $stage "new"
    $backup = Join-Path $stage "old"
    New-Item -ItemType Directory -Path $fresh, $backup -Force | Out-Null
    $names = @("runtime", "tui", "rg.exe")
    $names += "devo.exe"
    $changed = @()
    try {
        foreach ($name in @("devo.exe", "runtime\node\node.exe", "runtime\python\python.exe")) {
            $existing = Join-Path $InstallDir $name
            if (Test-Path -LiteralPath $existing) {
                $handle = [IO.File]::Open($existing, 'Open', 'ReadWrite', 'None')
                $handle.Dispose()
            }
        }
        foreach ($name in $names) { Copy-Item -LiteralPath (Join-Path $Source $name) -Destination (Join-Path $fresh $name) -Recurse }
        foreach ($name in $names) {
            $destination = Join-Path $InstallDir $name
            if (Test-Path -LiteralPath $destination) { Move-Item -LiteralPath $destination -Destination (Join-Path $backup $name) }
            $changed += $name
            Move-Item -LiteralPath (Join-Path $fresh $name) -Destination $destination
        }
    } catch {
        [array]::Reverse($changed)
        foreach ($name in $changed) {
            $destination = Join-Path $InstallDir $name
            if (Test-Path -LiteralPath $destination) { Remove-Item -LiteralPath $destination -Recurse }
            $previous = Join-Path $backup $name
            if (Test-Path -LiteralPath $previous) { Move-Item -LiteralPath $previous -Destination $destination }
        }
        throw "Devo installation failed: $($_.Exception.Message). Close running Devo apps and terminals, then retry."
    } finally {
        if ([IO.Path]::GetFullPath($stage).StartsWith([IO.Path]::GetFullPath($InstallDir) + [IO.Path]::DirectorySeparatorChar)) {
            Remove-Item -LiteralPath $stage -Recurse -ErrorAction SilentlyContinue
        }
    }
}

function Install-DevoOffline {
    param(
        [string]$AssetDir,
        [string]$InstallDir,
        [string]$TempRoot
    )

    $localExe = Join-Path $AssetDir "devo.exe"
    if (Test-Path $localExe) {
        Install-DevoBundle -Source $AssetDir -InstallDir $InstallDir
        return
    }

    $target = Get-Target
    $archive = Get-FirstMatchingFile -Directory $AssetDir -Pattern "devo-tui-*-${target}.zip"
    if (-not $archive) {
        Write-Error "Offline devo asset not found. Place devo-tui-*-${target}.zip or devo.exe next to install.ps1."
    }

    Write-Host "Installing devo from offline archive: $($archive.FullName)"
    $checksumFile = Join-Path $AssetDir "SHA256SUMS.txt"
    if (Test-Path -LiteralPath $checksumFile) { Test-ArchiveChecksum -Archive $archive.FullName -ChecksumFile $checksumFile -AssetName $archive.Name }
    $devoTmpDir = Join-Path $TempRoot "devo-offline"
    New-Item -ItemType Directory -Force -Path $devoTmpDir | Out-Null
    Expand-Archive -Path $archive.FullName -DestinationPath $devoTmpDir -Force

    $exe = Get-ChildItem -Recurse -Filter "devo.exe" -Path $devoTmpDir | Select-Object -First 1
    if (-not $exe) {
        Write-Error "devo.exe not found in the offline archive"
    }

    Install-DevoBundle -Source $exe.DirectoryName -InstallDir $InstallDir
}

function Install-RipgrepSidecarOffline {
    param(
        [string]$AssetDir,
        [string]$InstallDir,
        [string]$TempRoot
    )

    if ($env:DEVO_SKIP_RG_INSTALL -eq "1") {
        Write-Host "Skipping ripgrep sidecar install because DEVO_SKIP_RG_INSTALL=1."
        return
    }

    $targetPath = Join-Path $InstallDir "rg.exe"
    if (Test-Path $targetPath) {
        Write-Host "ripgrep sidecar is already installed at $targetPath"
        return
    }

    $localRg = Join-Path $AssetDir "rg.exe"
    if (Test-Path $localRg) {
        Write-Host "Installing ripgrep sidecar from $localRg"
        New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
        Copy-Item -Path $localRg -Destination $targetPath -Force
        return
    }

    $rgTarget = Get-RipgrepTarget
    $archive = Get-FirstMatchingFile -Directory $AssetDir -Pattern "ripgrep-*-${rgTarget}.zip"
    if (-not $archive) {
        Write-Error "Offline ripgrep asset not found. Place ripgrep-*-${rgTarget}.zip or rg.exe next to install.ps1."
    }

    Write-Host "Installing ripgrep sidecar from offline archive: $($archive.FullName)"
    $rgTmpDir = Join-Path $TempRoot "ripgrep-offline"
    New-Item -ItemType Directory -Force -Path $rgTmpDir | Out-Null
    Expand-Archive -Path $archive.FullName -DestinationPath $rgTmpDir -Force

    $rgExe = Get-ChildItem -Recurse -Filter "rg.exe" -Path $rgTmpDir | Select-Object -First 1
    if (-not $rgExe) {
        Write-Error "rg.exe not found in the offline archive"
    }

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Copy-Item -Path $rgExe.FullName -Destination $targetPath -Force
}



# ── Install ──────────────────────────────────────────────────────────────
function Main {
    Print-Banner

    $tmpDir = Join-Path $env:TEMP ("devo-install-" + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Force -Path $tmpDir | Out-Null

    try {
        $installDir = if ($env:DEVO_INSTALL_DIR) { $env:DEVO_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "Programs\devo" }

        if ($Offline) {
            $assetDir = Get-InstallerAssetDir
            Write-Host "Offline asset directory: $assetDir"
            Install-DevoOffline -AssetDir $assetDir -InstallDir $installDir -TempRoot $tmpDir
            Install-RipgrepSidecarOffline -AssetDir $assetDir -InstallDir $installDir -TempRoot $tmpDir
        } else {
            $target = Get-Target
            $version = Resolve-Version
            Write-VersionTransition -InstallDir $installDir -TargetVersion $version

            $skipAppInstall = Test-DevoVersionInstalled -InstallDir $installDir -ExpectedVersion $version
            if (-not $skipAppInstall) {
                $archiveName = "devo-tui-${version}-${target}.zip"
                $archiveUrl = "https://github.com/$Repo/releases/download/$version/$archiveName"

                Write-Host "Downloading devo $version for $target ..."

                $zipPath = Join-Path $tmpDir "devo.zip"
                Invoke-WebRequest -Uri $archiveUrl -OutFile $zipPath
                $checksumFile = Join-Path $tmpDir "SHA256SUMS.txt"
                Invoke-WebRequest -Uri "https://github.com/$Repo/releases/download/$version/SHA256SUMS.txt" -OutFile $checksumFile
                Test-ArchiveChecksum -Archive $zipPath -ChecksumFile $checksumFile -AssetName $archiveName

                Expand-Archive -Path $zipPath -DestinationPath $tmpDir -Force

                # Locate devo.exe (it's inside a versioned subdirectory).
                $exe = Get-ChildItem -Recurse -Filter "devo.exe" -Path $tmpDir | Select-Object -First 1
                if (-not $exe) {
                    Write-Error "devo.exe not found in the archive"
                }

                Install-DevoBundle -Source $exe.DirectoryName -InstallDir $installDir
            }
            Install-RipgrepSidecar -InstallDir $installDir -TempRoot $tmpDir
        }

        if ($env:DEVO_NO_MODIFY_PATH -ne "1") { Add-InstallDirToPath -InstallDir $installDir }

        Write-Host "Installed devo to ${installDir}\devo.exe"
        $rgPath = Join-Path $installDir "rg.exe"
        if (Test-Path $rgPath) {
            Write-Host "ripgrep sidecar available at $rgPath"
        } else {
            Write-Host "ripgrep sidecar was not installed."
        }
        Write-Host "PATH was updated for future terminals."
        Write-Host "Open a new terminal, or run:"
        Write-Host "  `$env:Path = `"$installDir;`$env:Path`""
        Write-Host "Run 'devo onboard' to get started."
    }
    finally {
        if ([IO.Path]::GetFullPath($tmpDir).StartsWith([IO.Path]::GetFullPath($env:TEMP) + [IO.Path]::DirectorySeparatorChar)) {
            Remove-Item -LiteralPath $tmpDir -Recurse -ErrorAction SilentlyContinue
        }
    }
}

Main

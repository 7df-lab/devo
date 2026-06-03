# install.ps1 — Download and install the latest devo binary for Windows.
#
# Usage (run as administrator is not required, installs to user-local bin):
#   irm https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1 | iex
#
# Pin a specific version:
#   $env:VERSION = "v0.1.2"; irm https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1 | iex

$ErrorActionPreference = "Stop"
$Repo = "7df-lab/devo"

# ── Platform detection ───────────────────────────────────────────────────
function Get-Target {
    $arch = if ([Environment]::Is64BitOperatingSystem) { "x86_64" } else {
        Write-Error "32-bit Windows is not supported"
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
function Resolve-Version {
    if ($env:VERSION) {
        return $env:VERSION
    }

    $latest = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest"
    return $latest.tag_name
}

# ── Banner ───────────────────────────────────────────────────────────────
function Print-Banner {
    Write-Host ""
    Write-Host "██████╗  ███████╗██╗   ██╗ ██████╗" -ForegroundColor DarkGray
    Write-Host "██╔══██╗ ██╔════╝██║   ██║██╔═══██╗" -ForegroundColor DarkGray
    Write-Host "██║  ██║ █████╗  ██║   ██║██║   ██║" -ForegroundColor DarkGray
    Write-Host "██║  ██║ ██╔══╝  ╚██╗ ██╔╝██║   ██║" -ForegroundColor DarkGray
    Write-Host "██████╔╝ ███████╗ ╚████╔╝ ╚██████╔╝" -ForegroundColor DarkGray
    Write-Host "╚═════╝  ╚══════╝  ╚═══╝   ╚═════╝" -ForegroundColor DarkGray
    Write-Host ""
}

# ── Install ──────────────────────────────────────────────────────────────
function Main {
    Print-Banner

    $target = Get-Target
    $version = Resolve-Version
    $archiveUrl = "https://github.com/$Repo/releases/download/$version/devo-${version}-${target}.zip"

    Write-Host "Downloading devo $version for $target ..."

    $tmpDir = Join-Path $env:TEMP "devo-install"
    Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue | Out-Null
    New-Item -ItemType Directory -Force -Path $tmpDir | Out-Null

    try {
        $zipPath = Join-Path $tmpDir "devo.zip"
        Invoke-WebRequest -Uri $archiveUrl -OutFile $zipPath

        Expand-Archive -Path $zipPath -DestinationPath $tmpDir -Force

        # Locate devo.exe (it's inside a versioned subdirectory).
        $exe = Get-ChildItem -Recurse -Filter "devo.exe" -Path $tmpDir | Select-Object -First 1
        if (-not $exe) {
            Write-Error "devo.exe not found in the archive"
        }

        # Install target.
        $installDir = Join-Path $env:LOCALAPPDATA "Programs\devo"
        New-Item -ItemType Directory -Force -Path $installDir | Out-Null
        Copy-Item -Path $exe.FullName -Destination (Join-Path $installDir "devo.exe") -Force

        Add-InstallDirToPath -InstallDir $installDir

        Write-Host "Installed devo to ${installDir}\devo.exe"
        Write-Host "PATH was updated for future terminals."
        Write-Host "Open a new terminal, or run:"
        Write-Host "  `$env:Path = `"$installDir;`$env:Path`""
        Write-Host "Run 'devo onboard' to get started."
    }
    finally {
        Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue | Out-Null
    }
}

Main

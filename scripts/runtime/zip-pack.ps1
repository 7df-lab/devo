param(
    [Parameter(Mandatory = $true)][string]$Root,
    [Parameter(Mandatory = $true)][string]$Output,
    [Parameter(Mandatory = $true)][string]$Manifest
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem

$rootFull = [IO.Path]::GetFullPath($Root)
$manifestText = [IO.File]::ReadAllText($Manifest, [Text.Encoding]::UTF8)
$paths = @(ConvertFrom-Json -InputObject $manifestText)
if ($paths.Count -eq 0) { throw "No pack paths were provided" }

$entries = [System.Collections.Generic.List[object]]::new()
$seen = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)

function Add-ZipEntry {
    param([string]$FullPath, [string]$Relative, [bool]$Directory)
    $name = $Relative.Replace('\', '/')
    while ($name.StartsWith("./")) { $name = $name.Substring(2) }
    if ($name -eq "" -or $name.StartsWith("/") -or ($name.Split('/') -contains "..")) {
        throw "Invalid runtime pack path: $Relative"
    }
    if ($Directory) { $name = $name.TrimEnd('/') + "/" }
    if (-not $seen.Add($name)) { throw "Duplicate runtime pack entry: $name" }
    $entries.Add([pscustomobject]@{ FullPath = $FullPath; Name = $name; Directory = $Directory })
}

function Add-Tree {
    param([string]$FullPath, [string]$Relative)
    $item = Get-Item -LiteralPath $FullPath -Force -ErrorAction Stop
    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Unsupported reparse point in Windows runtime pack: $Relative"
    }
    if ($item.PSIsContainer) {
        Add-ZipEntry -FullPath $item.FullName -Relative $Relative -Directory $true
        foreach ($child in @(Get-ChildItem -LiteralPath $item.FullName -Force | Sort-Object Name)) {
            Add-Tree -FullPath $child.FullName -Relative ($Relative.TrimEnd('/') + "/" + $child.Name)
        }
    } else {
        Add-ZipEntry -FullPath $item.FullName -Relative $Relative -Directory $false
    }
}

foreach ($path in $paths) {
    Add-Tree -FullPath (Join-Path $rootFull $path) -Relative $path
}

$byName = [System.Collections.Generic.Dictionary[string, object]]::new([StringComparer]::Ordinal)
foreach ($entry in $entries) { $byName[$entry.Name] = $entry }
$names = [string[]]@($byName.Keys)
[Array]::Sort($names, [StringComparer]::Ordinal)
$sorted = foreach ($name in $names) { $byName[$name] }

$stream = [IO.File]::Open($Output, [IO.FileMode]::Create, [IO.FileAccess]::Write, [IO.FileShare]::None)
try {
    $utf8 = [System.Text.UTF8Encoding]::new($false)
    $archive = New-Object -TypeName System.IO.Compression.ZipArchive -ArgumentList @($stream, [IO.Compression.ZipArchiveMode]::Create, $true, $utf8)
    try {
        # Fixed DOS timestamp keeps repeated Windows pack builds byte-identical
        # within the supported .NET toolchain.
        $fixed = [DateTimeOffset]::new(1980, 1, 1, 0, 0, 0, [TimeSpan]::Zero)
        foreach ($entry in $sorted) {
            # ZipArchive writes UTF-8 entry names (and the UTF-8 flag) when a
            # name contains non-ASCII characters.
            $zipEntry = $archive.CreateEntry($entry.Name, [IO.Compression.CompressionLevel]::Optimal)
            $zipEntry.LastWriteTime = $fixed
            if (-not $entry.Directory) {
                $input = [IO.File]::OpenRead($entry.FullPath)
                try {
                    $outputStream = $zipEntry.Open()
                    try { $input.CopyTo($outputStream) } finally { $outputStream.Dispose() }
                } finally { $input.Dispose() }
            }
        }
    } finally {
        $archive.Dispose()
    }
} finally {
    $stream.Dispose()
}

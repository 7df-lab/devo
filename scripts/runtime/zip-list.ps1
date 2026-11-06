param(
    [Parameter(Mandatory = $true)][string]$Archive,
    [Parameter(Mandatory = $true)][string]$OutputFile
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [IO.Compression.ZipFile]::OpenRead($Archive)
try {
    $names = [System.Collections.Generic.List[string]]::new()
    foreach ($entry in $zip.Entries) { $names.Add($entry.FullName) }
    [IO.File]::WriteAllLines($OutputFile, $names, [System.Text.UTF8Encoding]::new($false))
} finally {
    $zip.Dispose()
}

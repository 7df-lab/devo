param(
    [Parameter(Mandatory = $true)][string]$Archive,
    [Parameter(Mandatory = $true)][string]$Manifest,
    [Parameter(Mandatory = $true)][string]$OutputFile
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem

function Hex-Bytes([byte[]]$Bytes) {
    ($Bytes | ForEach-Object { $_.ToString("X2") }) -join ""
}

$lines = [System.Collections.Generic.List[string]]::new()
$manifestText = [IO.File]::ReadAllText($Manifest, [Text.Encoding]::UTF8)
$parsed = ConvertFrom-Json -InputObject $manifestText
$manifestPath = if ($parsed -is [System.Array]) { [string]$parsed[0] } else { [string]$parsed }
$manifestPath = $manifestPath.Replace('\', '/')
$lines.Add("MANIFEST_UTF8_HEX=" + (Hex-Bytes ([Text.Encoding]::UTF8.GetBytes($manifestPath))))

$stream = [IO.File]::OpenRead($Archive)
try {
    $defaultZip = New-Object -TypeName System.IO.Compression.ZipArchive -ArgumentList @($stream, [IO.Compression.ZipArchiveMode]::Read, $true)
    try {
        foreach ($entry in $defaultZip.Entries) {
            $lines.Add("DEFAULT_NAME=" + $entry.FullName)
            $lines.Add("DEFAULT_UTF8_HEX=" + (Hex-Bytes ([Text.Encoding]::UTF8.GetBytes($entry.FullName))))
        }
    } finally { $defaultZip.Dispose() }
} finally { $stream.Dispose() }

$stream = [IO.File]::OpenRead($Archive)
try {
    $utf8Zip = New-Object -TypeName System.IO.Compression.ZipArchive -ArgumentList @($stream, [IO.Compression.ZipArchiveMode]::Read, $true, [Text.Encoding]::UTF8)
    try {
        foreach ($entry in $utf8Zip.Entries) {
            $lines.Add("UTF8_NAME=" + $entry.FullName)
            $lines.Add("UTF8_HEX=" + (Hex-Bytes ([Text.Encoding]::UTF8.GetBytes($entry.FullName))))
        }
    } finally { $utf8Zip.Dispose() }
} finally { $stream.Dispose() }

$bytes = [IO.File]::ReadAllBytes($Archive)
for ($i = 0; $i -le $bytes.Length - 30; $i++) {
    if ($bytes[$i] -eq 0x50 -and $bytes[$i + 1] -eq 0x4b -and $bytes[$i + 2] -eq 0x03 -and $bytes[$i + 3] -eq 0x04) {
        $flags = [BitConverter]::ToUInt16($bytes, $i + 6)
        $nameLength = [BitConverter]::ToUInt16($bytes, $i + 26)
        if ($nameLength -gt 0 -and ($i + 30 + $nameLength) -le $bytes.Length) {
            $nameBytes = New-Object byte[] $nameLength
            [Array]::Copy($bytes, $i + 30, $nameBytes, 0, $nameLength)
            $lines.Add("RAW_FLAGS=0x" + $flags.ToString("X4"))
            $lines.Add("RAW_NAME_UTF8=" + [Text.Encoding]::UTF8.GetString($nameBytes))
            $lines.Add("RAW_NAME_HEX=" + (Hex-Bytes $nameBytes))
        }
    }
}
[IO.File]::WriteAllLines($OutputFile, $lines, [System.Text.UTF8Encoding]::new($false))

# Manual E2E helper: generate a stress corpus, validate JSONL, optionally bench RPCs,
# then print how to launch TUI/desktop against it.
#
# Example (medium load):
#   pwsh scripts/stress-session-e2e.ps1 -OutDir $env:TEMP\devo-stress -Roots 100 -Subagents 50
#
# Then:
#   $env:DEVO_HOME = "<OutDir>"
#   .\target\debug\devo.exe

param(
    [Parameter(Mandatory = $true)]
    [string]$OutDir,

    [int]$Roots = 100,
    [int]$Subagents = 50,
    [int]$Turns = 15,
    [int]$MessagesPerTurn = 3,
    [int]$CommRounds = 40,
    [int]$MessageBytes = 512,
    [switch]$Force,
    [switch]$SkipBench,
    [string]$DevoBin = ".\target\debug\devo.exe"
)

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
Set-Location $repo

Write-Host "Building devo-stress + devo..."
cargo build -p devo-stress-tools -p devo-cli --bins
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$stress = ".\target\debug\devo-stress.exe"
$forceArgs = @()
if ($Force) { $forceArgs += "--force" }

& $stress generate `
    --out $OutDir `
    --roots $Roots `
    --subagents $Subagents `
    --turns $Turns `
    --messages-per-turn $MessagesPerTurn `
    --comm-rounds $CommRounds `
    --message-bytes $MessageBytes `
    @forceArgs
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

& $stress validate --corpus $OutDir
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

if (-not $SkipBench) {
    if (-not (Test-Path $DevoBin)) {
        Write-Warning "Devo binary not found at $DevoBin; skipping bench"
    } else {
        & $stress bench --corpus $OutDir --devo-bin $DevoBin --samples 3 --page-size 50
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    }
}

$manifest = Get-Content (Join-Path $OutDir "stress-manifest.json") -Raw | ConvertFrom-Json
Write-Host ""
Write-Host "Manual UX check:"
Write-Host "  `$env:DEVO_HOME = '$OutDir'"
Write-Host "  $DevoBin"
Write-Host "Hot root id: $($manifest.hot_root_id)"
Write-Host "Look for sluggish session list, Agents View with children, resume of hot root, items scroll."

# Compatibility entry point; assertions and cleanup are shared across platforms.
param([string]$Scenario = "T1")
& node (Join-Path $PSScriptRoot "run-tmux-smoke.js") $Scenario
exit $LASTEXITCODE

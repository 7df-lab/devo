# Minimal InteractiveMode smoke under psmux/tmux (Windows).
# Usage: powershell -File scripts/tmux-tui-smoke.ps1 [T1|T2|T3|goal|bash|agents|schedule|heartbeat]
# Markers only — do not assert exact model reply text.
param(
  [string]$Scenario = "T1"
)

$ErrorActionPreference = "Stop"
$Session = if ($env:DEVO_TUI_TEST_SESSION) { $env:DEVO_TUI_TEST_SESSION } else { "devo-tui-test" }
$tmux = if ($env:DEVO_TMUX_BIN) { $env:DEVO_TMUX_BIN } else { "psmux" }
$Root = Split-Path -Parent $PSScriptRoot
$Repo = Split-Path -Parent (Split-Path -Parent $Root)
$Devo = Join-Path $Repo "target\debug\devo.exe"
if (-not (Test-Path $Devo)) {
  Write-Error "Build devo first: cargo build -p devo-cli"
}

function Cleanup {
  & $tmux kill-session -t $Session 2>$null
}
trap { Cleanup }

# Prefer -- before the binary (psmux requires it for a custom command).
Cleanup
& $tmux new-session -d -s $Session -x 100 -y 30 -- $Devo
Start-Sleep -Seconds 4
$pane = & $tmux capture-pane -t $Session -p 2>$null
if (-not $pane) {
  # Fallback: some psmux builds list the session but fail capture briefly.
  Start-Sleep -Seconds 1
  $pane = & $tmux capture-pane -t $Session -p 2>$null
}
Write-Host "=== boot ==="
Write-Host $pane

switch ($Scenario) {
  "T1" {
    if ($pane -notmatch "devo|prime|agent|/" ) {
      Write-Warning "T1: no clear idle chrome yet (fullscreen may hide boot text)"
    }
    Write-Host "T1 ok (boot captured)"
  }
  "T2" {
    & $tmux send-keys -t $Session "Say hello in one short sentence." Enter
    Start-Sleep -Seconds 25
    $pane2 = & $tmux capture-pane -t $Session -p
    Write-Host $pane2
    Write-Host "T2 ok (prompt sent)"
  }
  "T3" {
    & $tmux send-keys -t $Session "Write a long reply." Enter
    Start-Sleep -Seconds 3
    & $tmux send-keys -t $Session Escape
    Start-Sleep -Seconds 2
    Write-Host "T3 ok (interrupt attempted)"
  }
  "goal" {
    & $tmux send-keys -t $Session "/goal" Enter
    Start-Sleep -Seconds 2
    & $tmux send-keys -t $Session "/goal smoke objective" Enter
    Start-Sleep -Seconds 3
    $paneG = & $tmux capture-pane -t $Session -p
    Write-Host $paneG
    Write-Host "goal ok (slash sent)"
  }
  "bash" {
    & $tmux send-keys -t $Session "!echo smoke-bash" Enter
    Start-Sleep -Seconds 4
    $paneB = & $tmux capture-pane -t $Session -p
    Write-Host $paneB
    Write-Host "bash ok (command sent)"
  }
  "agents" {
    & $tmux send-keys -t $Session Escape
    Start-Sleep -Seconds 1
    & $tmux send-keys -t $Session C-o
    Start-Sleep -Seconds 2
    $paneA = & $tmux capture-pane -t $Session -p
    Write-Host $paneA
    Write-Host "agents ok (view toggle attempted)"
  }
  "schedule" {
    & $tmux send-keys -t $Session "/heartbeat" Enter
    Start-Sleep -Seconds 2
    $paneS = & $tmux capture-pane -t $Session -p
    Write-Host $paneS
    Write-Host "schedule ok (heartbeat slash sent)"
  }
  "heartbeat" {
    # Full fire-and-deliver e2e under a real TTY. Reboot the session under an
    # isolated DEVO_HOME: a firing 1s heartbeat writes the process-shared
    # schedules.json, which must never be the user's. settings.json marks
    # onboarding shown so a fresh home boots into chat.
    $hbHome = Join-Path ([System.IO.Path]::GetTempPath()) ("devo-tui-heartbeat-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
    New-Item -ItemType Directory -Path $hbHome -Force | Out-Null
    Set-Content -Path (Join-Path $hbHome "settings.json") -Value '{"onboardingShown":true}'
    & $tmux kill-session -t $Session 2>$null
    $env:DEVO_HOME = $hbHome
    & $tmux new-session -d -s $Session -x 100 -y 30 -- $Devo
    Start-Sleep -Seconds 5
    $probe = "hb-tui-fire-" + [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
    & $tmux send-keys -t $Session "/heartbeat every 1s $probe" Enter
    # The server schedule wake loop ticks every 15s; poll for the delivered
    # heartbeat prompt instead of a fixed sleep so slow CI cannot flake.
    $fired = $false
    $deadline = [DateTimeOffset]::UtcNow.AddSeconds(60)
    while ([DateTimeOffset]::UtcNow -lt $deadline) {
      $paneH = & $tmux capture-pane -t $Session -p 2>$null
      if ($paneH -and ($paneH -match [regex]::Escape($probe))) { $fired = $true; break }
      Start-Sleep -Seconds 5
    }
    Write-Host $paneH
    if (-not $fired) {
      $env:DEVO_HOME = $null
      Remove-Item -Recurse -Force $hbHome -ErrorAction SilentlyContinue
      Write-Error "heartbeat fire not visible in pane (probe=$probe)"
    }
    Write-Host "heartbeat fired and delivered ($probe)"
    & $tmux send-keys -t $Session "/heartbeat clear" Enter
    Start-Sleep -Seconds 3
    $paneC = & $tmux capture-pane -t $Session -p
    Write-Host $paneC
    $env:DEVO_HOME = $null
    Remove-Item -Recurse -Force $hbHome -ErrorAction SilentlyContinue
    if (-not ($paneC -match "Heartbeat cleared")) {
      Write-Error "heartbeat clear not confirmed"
    }
    Write-Host "heartbeat ok (set, fired, cleared)"
  }
  default {
    Write-Host "Unknown scenario $Scenario — ran boot only"
  }
}

Cleanup

# Calendar read check against the real accounts on this device (WP 1.11).
#
# Read-only: it never creates, changes or deletes events or mail. It may
# restart famd, and with -Clean it deletes the local store and search index
# so the next sync starts from nothing (both are rebuilt from the server;
# config.toml and the keychain tokens are never touched).
#
#   pwsh -File scripts/cal-check.ps1                  # use the running famd, start it if needed
#   pwsh -File scripts/cal-check.ps1 -Build -Restart  # rebuild, then restart famd
#   pwsh -File scripts/cal-check.ps1 -Build -Clean    # rebuild, wipe the store, sync from scratch
#
# famd's stderr goes to target\cal-check\famd.log. famd is left running.
[CmdletBinding()]
param(
    [switch]$Build,
    [switch]$Restart,
    [switch]$Clean,
    [string]$Calendar = 'Family Room',
    [string]$From = '2026-12-01',
    [int]$WaitSeconds = 300
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$fam = Join-Path $root 'target\release\fam.exe'
$famd = Join-Path $root 'target\release\famd.exe'
$logDir = Join-Path $root 'target\cal-check'
$log = Join-Path $logDir 'famd.log'
$data = Join-Path $env:LOCALAPPDATA 'Familiar'

function Section($title) { Write-Output ''; Write-Output "=== $title" }
function Fam { & $fam @args 2>&1 | ForEach-Object { "$_" }; Write-Output "(exit $LASTEXITCODE)" }

if ($Build) {
    Section 'cargo build --release -p familiar-daemon -p familiar-cli'
    Push-Location $root
    try { cargo build --release -p familiar-daemon -p familiar-cli 2>&1 | Select-Object -Last 3 | ForEach-Object { "$_" } }
    finally { Pop-Location }
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }
}

Section 'git'
git -C $root log --oneline -1

$running = Get-Process famd -ErrorAction SilentlyContinue
if ($running -and ($Restart -or $Clean)) {
    Section "stopping famd (PID $($running.Id -join ', '))"
    $running | Stop-Process -Force
    $running | Wait-Process -Timeout 15 -ErrorAction SilentlyContinue
    $running = $null
}
if ($Clean) {
    Section "deleting store and search index under $data"
    foreach ($d in 'store', 'search') {
        $p = Join-Path $data $d
        if (Test-Path $p) { Remove-Item -Recurse -Force $p; Write-Output "removed $p" }
    }
}
if (-not $running) {
    New-Item -ItemType Directory -Force $logDir | Out-Null
    $p = Start-Process -FilePath $famd -RedirectStandardError $log -RedirectStandardOutput "$log.out" -WindowStyle Hidden -PassThru
    Section "started famd (PID $($p.Id)), log $log"
    Start-Sleep -Seconds 3
    if ($p.HasExited) { Get-Content $log; throw "famd exited with code $($p.ExitCode)" }
}

Section "waiting up to $WaitSeconds s for '$Calendar' to sync"
$deadline = (Get-Date).AddSeconds($WaitSeconds)
do {
    $cals = & $fam cal calendars 2>&1 | ForEach-Object { "$_" }
    if ($cals -match [regex]::Escape($Calendar) + '.*events from') { break }
    Start-Sleep -Seconds 10
} while ((Get-Date) -lt $deadline)

Section 'fam cal calendars'
Fam cal calendars

Section "fam cal list --calendar `"$Calendar`" --days 14"
Fam cal list --calendar $Calendar --days 14

Section "fam cal list --calendar `"$Calendar`" --days 14 --json (first 3 events)"
$json = & $fam cal list --calendar $Calendar --days 14 --json 2>&1 | Out-String
try {
    $parsed = $json | ConvertFrom-Json
    Write-Output "events: $($parsed.events.Count)"
    $parsed.events | Select-Object -First 3 | ConvertTo-Json -Depth 5
    $blank = @($parsed.events | Where-Object { -not $_.subject })
    Write-Output "events with a blank subject: $($blank.Count)"
} catch { Write-Output $json }

Section 'fam cal list --days 7 (every calendar)'
Fam cal list --days 7

Section "fam cal list --calendar `"$Calendar`" --from $From --days 31"
Fam cal list --calendar $Calendar --from $From --days 31

Section 'tzutil /g'
tzutil /g
Write-Output ''

Section "famd log: calendar lines and errors ($log)"
if (Test-Path $log) {
    Select-String -Path $log -Pattern 'calendar|error|warn|fail' | ForEach-Object { $_.Line }
} else {
    Write-Output 'no log here: famd was already running when this script started (use -Restart to log)'
}

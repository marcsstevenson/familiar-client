# WP 0.4 bake-off harness for Windows (the primary platform).
#
# Run from spikes\ui-bakeoff in PowerShell 7 or Windows PowerShell 5.1:
#   cd tauri; npm ci; npm run build; cd ..
#   cargo build --release
#   .\bench\run_windows.ps1 -Runs 5
#
# Each app is launched in bench mode several times. The app scrolls, moves
# the selection and opens messages by itself and logs timings; this script
# adds real key presses (SendKeys), idle memory across the whole process
# tree (so WebView2's helper processes count) and idle CPU, then writes
# bench\results\windows-<app>.json and prints a summary table.
param(
    [int]$Runs = 5,
    [string[]]$Apps = @("gpui", "gpui-webview", "tauri")
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root "target" }
$exe = @{ gpui = Join-Path $target "release\bakeoff-gpui.exe"; "gpui-webview" = Join-Path $target "release\bakeoff-gpui.exe"; tauri = Join-Path $target "release\bakeoff-tauri.exe" }
$htmlMode = @{ gpui = "native"; "gpui-webview" = "webview"; tauri = "" }
$title = @{ gpui = "Familiar (GPUI)"; "gpui-webview" = "Familiar (GPUI)"; tauri = "Familiar (Tauri)" }
$outDir = Join-Path $PSScriptRoot "results"
New-Item -ItemType Directory -Force $outDir | Out-Null
Add-Type -AssemblyName System.Windows.Forms
$wsh = New-Object -ComObject WScript.Shell

function EpochNs { ([DateTime]::UtcNow.Ticks - 621355968000000000) * 100 }
function Pct($xs, $p) {
    $s = @($xs | Sort-Object); if ($s.Count -eq 0) { return $null }
    $k = ($s.Count - 1) * $p / 100; $f = [math]::Floor($k); $c = [math]::Min($f + 1, $s.Count - 1)
    return $s[$f] + ($s[$c] - $s[$f]) * ($k - $f)
}
function Events($path) {
    if (-not (Test-Path $path)) { return @() }
    return @(Get-Content $path | Where-Object { $_.Trim() } | ForEach-Object { $_ | ConvertFrom-Json })
}
function WaitFor($path, $name, $timeoutS) {
    $end = (Get-Date).AddSeconds($timeoutS)
    while ((Get-Date) -lt $end) {
        if (Events $path | Where-Object { $_.event -eq $name }) { return $true }
        Start-Sleep -Milliseconds 50
    }
    return $false
}
function Tree($procId) {
    $all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId
    $out = @($procId); $todo = @($procId)
    while ($todo.Count) {
        $p = $todo[0]; $todo = @($todo | Select-Object -Skip 1)
        $kids = @($all | Where-Object { $_.ParentProcessId -eq $p } | ForEach-Object { $_.ProcessId })
        $out += $kids; $todo += $kids
    }
    return $out
}

$summaryRows = @()
foreach ($app in $Apps) {
    $runsOut = @()
    for ($i = 0; $i -lt $Runs; $i++) {
        $log = Join-Path $outDir "$app-win-run$i.jsonl"
        Remove-Item $log -ErrorAction SilentlyContinue
        $env:FAMILIAR_BENCH_OUT = $log
        $env:FAMILIAR_GPUI_HTML = $htmlMode[$app]
        $t0 = EpochNs
        $proc = Start-Process -FilePath $exe[$app] -PassThru
        try {
            if (-not (WaitFor $log "first_frame" 60)) { throw "${app}: no first frame" }
            $ev = Events $log
            $cold = ([long]($ev | Where-Object event -eq "first_frame" | Select-Object -First 1).t_ns - $t0) / 1e6
            if (-not (WaitFor $log "bench_done" 180)) { throw "${app}: bench did not finish" }
            $null = $wsh.AppActivate($title[$app]); Start-Sleep -Milliseconds 400
            $sent = @()
            for ($k = 0; $k -lt 30; $k++) {
                $sent += EpochNs
                [System.Windows.Forms.SendKeys]::SendWait("j")
                Start-Sleep -Milliseconds 150
            }
            Start-Sleep -Seconds 12
            $pids = Tree $proc.Id
            $procs = @($pids | ForEach-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue })
            $ws = ($procs | Measure-Object WorkingSet64 -Sum).Sum / 1MB
            $priv = ($procs | Measure-Object PrivateMemorySize64 -Sum).Sum / 1MB
            $c0 = ($procs | ForEach-Object { $_.Refresh(); $_.TotalProcessorTime.TotalMilliseconds } | Measure-Object -Sum).Sum
            Start-Sleep -Seconds 5
            $c1 = ($procs | ForEach-Object { $_.Refresh(); $_.TotalProcessorTime.TotalMilliseconds } | Measure-Object -Sum).Sum
            $ev = Events $log
            $keys = @($ev | Where-Object event -eq "key")
            $ext = @()
            if ($keys.Count -eq $sent.Count) { for ($k = 0; $k -lt $keys.Count; $k++) { $ext += ([long]$keys[$k].t_ns - $sent[$k]) / 1e6 } }
            $scroll = $ev | Where-Object event -eq "scroll" | Select-Object -First 1
            $runsOut += [pscustomobject]@{
                run = $i; cold_start_ms = $cold; processes = $pids.Count
                idle_working_set_mb = $ws; idle_private_mb = $priv
                idle_cpu_pct = ($c1 - $c0) / 5000 * 100  # percent of one core
                scroll_frame_ms = @($scroll.frame_ms); blank_frames = $scroll.blank_frames
                self_key_ms = @($ev | Where-Object event -eq "self_key" | ForEach-Object latency_ms)
                key_in_app_ms = @($keys | ForEach-Object latency_ms)
                key_external_ms = $ext
                open_ms = @($ev | Where-Object event -eq "open" | ForEach-Object latency_ms)
                sanitize_ms = @($ev | Where-Object event -eq "open" | ForEach-Object sanitize_ms)
            }
            Write-Host ("{0} run {1}: start {2:N0} ms, working set {3:N0} MB, private {4:N0} MB" -f $app, $i, $cold, $ws, $priv)
        } finally {
            Tree $proc.Id | ForEach-Object { Stop-Process -Id $_ -Force -ErrorAction SilentlyContinue }
        }
    }
    $frames = @($runsOut | ForEach-Object { $_.scroll_frame_ms })
    $flat = { param($k) @($runsOut | ForEach-Object { $_.$k }) }
    $s = [pscustomobject]@{
        app = $app; runs = $Runs
        cold_start_median_ms = Pct ($runsOut.cold_start_ms) 50
        idle_working_set_mb = Pct ($runsOut.idle_working_set_mb) 50
        idle_private_mb = Pct ($runsOut.idle_private_mb) 50
        idle_cpu_pct = Pct ($runsOut.idle_cpu_pct) 50
        processes = $runsOut[0].processes
        scroll_p50_ms = Pct $frames 50; scroll_p95_ms = Pct $frames 95; scroll_p99_ms = Pct $frames 99
        scroll_over_16_7_pct = 100 * @($frames | Where-Object { $_ -gt 16.7 }).Count / [math]::Max(1, $frames.Count)
        self_key_p95_ms = Pct (& $flat "self_key_ms") 95
        key_external_p95_ms = Pct (& $flat "key_external_ms") 95
        open_p50_ms = Pct (& $flat "open_ms") 50; open_p95_ms = Pct (& $flat "open_ms") 95
        binary_mb = (Get-Item $exe[$app]).Length / 1MB
    }
    $summaryRows += $s
    @{ summary = $s; runs = $runsOut } | ConvertTo-Json -Depth 6 | Set-Content (Join-Path $outDir "windows-$app.json")
}
$summaryRows | Format-List
Write-Host "Results written to $outDir. Paste the windows-*.json summaries into the thread."

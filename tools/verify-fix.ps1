"=== launch headless ==="
Start-Process -FilePath 'E:\Project\Ocypus Display rust\target\release\ocydash.exe'
Start-Sleep -Seconds 6

# newest instance = the test-mutex build we just launched
$p = Get-Process ocydash -ErrorAction Stop | Sort-Object StartTime -Descending | Select-Object -First 1
"pid $($p.Id)  RSS $([math]::Round($p.WorkingSet64/1MB,1))MB  priv $([math]::Round($p.PrivateMemorySize64/1MB,1))MB"
$c1 = $p.TotalProcessorTime.TotalSeconds

$ff = Get-Process ffmpeg -ErrorAction SilentlyContinue | Sort-Object StartTime -Descending | Select-Object -First 1
if ($ff) { "newest ffmpeg: pid $($ff.Id) started $($ff.StartTime.ToString('HH:mm:ss'))" } else { "no ffmpeg" }

"=== 15s steady-state CPU sample ==="
Start-Sleep -Seconds 15
$p.Refresh()
$c2 = $p.TotalProcessorTime.TotalSeconds
"cpu delta over 15s: {0:N2}s  = {1:N2}% of one core" -f ($c2-$c1), (($c2-$c1)/15*100)
"RSS now: {0:N1}MB  priv {1:N1}MB" -f ($p.WorkingSet64/1MB), ($p.PrivateMemorySize64/1MB)

"=== force-kill parent, check job object reaps ffmpeg ==="
$ffId = if ($ff) { $ff.Id } else { 0 }
Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2
$still = Get-Process -Id $ffId -ErrorAction SilentlyContinue
if ($still) { "FAIL: ffmpeg $ffId survived parent death" } else { "OK: ffmpeg $ffId reaped by job object" }

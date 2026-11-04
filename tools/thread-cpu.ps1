$p = Get-Process ocydash -ErrorAction Stop
$t1 = @{}
$p.Threads | ForEach-Object { $t1[$_.Id] = $_.TotalProcessorTime.TotalSeconds }
Start-Sleep -Seconds 4
$p.Refresh()
"threads: $($p.Threads.Count) | RSS: $([math]::Round($p.WorkingSet64/1MB,1))MB"
$p.Threads | ForEach-Object {
    $d = $_.TotalProcessorTime.TotalSeconds - $t1[$_.Id]
    "tid {0,-8} +{1,7:N3}s" -f $_.Id, $d
} | Sort-Object

$names = @('ocydash','ffmpeg','SystemInfos')
foreach ($n in $names) {
    $ps = Get-Process $n -ErrorAction SilentlyContinue
    foreach ($p in $ps) {
        "{0,-12} pid {1,-6} RSS {2,6:N1}MB  priv {3,6:N1}MB  cpu {4,7:N1}s" -f `
            $n, $p.Id, ($p.WorkingSet64/1MB), ($p.PrivateMemorySize64/1MB), `
            $p.TotalProcessorTime.TotalSeconds
    }
}
Start-Sleep -Seconds 45
"--- after 45s ---"
foreach ($n in $names) {
    $ps = Get-Process $n -ErrorAction SilentlyContinue
    foreach ($p in $ps) {
        "{0,-12} pid {1,-6} RSS {2,6:N1}MB  priv {3,6:N1}MB  cpu {4,7:N1}s" -f `
            $n, $p.Id, ($p.WorkingSet64/1MB), ($p.PrivateMemorySize64/1MB), `
            $p.TotalProcessorTime.TotalSeconds
    }
}

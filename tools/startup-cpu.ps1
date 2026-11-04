Start-Process -FilePath 'E:\Project\Ocypus Display rust\target\release\ocydash.exe'
Start-Sleep -Milliseconds 800
for ($i = 0; $i -lt 12; $i++) {
    $p = Get-Process ocydash -ErrorAction SilentlyContinue
    if (-not $p) { break }
    $t = $p.TotalProcessorTime.TotalSeconds
    "t={0,4:N1}s  cpu {1,7:N1}s  RSS {2,5:N1}MB  threads {3}" -f `
        ($i * 1.0), $t, ($p.WorkingSet64/1MB), $p.Threads.Count
    Start-Sleep -Seconds 1
}

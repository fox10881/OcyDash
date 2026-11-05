$queries = @(
    'Win32_SoundDevice|',
    'Win32_NetworkAdapter|PhysicalAdapter=True AND NOT Manufacturer LIKE ''%Microsoft%''',
    'WmiMonitorID|root\wmi|Active=True',
    'Win32_SystemSlot|',
    'Win32_VideoController|'
)
foreach ($q in $queries) {
    $p = $q -split '\|'; $cls=$p[0]; $filt=$p[1]; $ns= if($p.Count -gt 2){$p[2]}else{'root\cimv2'}
    $sw=[Diagnostics.Stopwatch]::StartNew()
    $job = Start-Job -ScriptBlock {
        param($cls,$filt,$ns)
        try {
            if($filt){Get-CimInstance -ClassName $cls -Namespace $ns -Filter $filt -ErrorAction Stop|Select -First 1|Out-Null}
            else{Get-CimInstance -ClassName $cls -Namespace $ns -ErrorAction Stop|Select -First 1|Out-Null}
            'OK'
        } catch { "ERR: $($_.Exception.Message)" }
    } -ArgumentList $cls,$filt,$ns
    $r = Wait-Job $job -Timeout 15; $sw.Stop()
    if($r){"{0,7:N0}ms  {1,-55} {2}" -f $sw.ElapsedMilliseconds,"$cls($ns)",(Receive-Job $job)}
    else{"{0,7:N0}ms  {1,-55} TIMEOUT/HUNG" -f $sw.ElapsedMilliseconds,"$cls($ns)"; Stop-Job $job}
    Remove-Job $job -Force
}

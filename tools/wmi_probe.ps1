$classes = @(
    'Win32_PnPEntity|PNPDeviceID like ''PCI\\VEN_10DE%''',
    'Win32_PnPEntity|PNPDeviceID like ''PCI\\VEN_1002%''',
    'Win32_VideoController|',
    'Win32_DesktopMonitor|',
    'Win32_Processor|',
    'Win32_PhysicalMemory|',
    'Win32_Fan|',
    'Win32_TemperatureProbe|',
    'Win32_BaseBoard|',
    'Win32_DiskDrive|',
    'CIM_NetworkAdapter|',
    'MSStorageDriver_FailurePredictStatus|root\wmi',
    'MSAcpi_ThermalZoneTemperature|root\wmi'
)
foreach ($c in $classes) {
    $parts = $c -split '\|'
    $cls = $parts[0]; $filter = $parts[1]; $ns = if ($parts[2]) { $parts[2] } else { 'root\cimv2' }
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $job = Start-Job -ScriptBlock {
        param($cls,$filter,$ns)
        try {
            if ($filter) { Get-CimInstance -ClassName $cls -Namespace $ns -Filter $filter -ErrorAction Stop | Select -First 1 | Out-Null }
            else { Get-CimInstance -ClassName $cls -Namespace $ns -ErrorAction Stop | Select -First 1 | Out-Null }
            'OK'
        } catch { "ERR: $($_.Exception.Message)" }
    } -ArgumentList $cls,$filter,$ns
    $r = Wait-Job $job -Timeout 15
    $sw.Stop()
    if ($r) { "{0,7:N0}ms  {1,-45} {2}" -f $sw.ElapsedMilliseconds, "$cls($ns)", (Receive-Job $job) }
    else    { "{0,7:N0}ms  {1,-45} TIMEOUT/HUNG" -f $sw.ElapsedMilliseconds, "$cls($ns)"; Stop-Job $job }
    Remove-Job $job -Force
}

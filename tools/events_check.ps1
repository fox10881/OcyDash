$start = Get-Date "2026-11-05 02:30"
$end   = Get-Date "2026-11-05 03:10"
"=== System log 02:30-03:10 (Warning/Error/Information, first 40) ==="
Get-WinEvent -FilterHashtable @{LogName='System'; StartTime=$start; EndTime=$end} -ErrorAction SilentlyContinue |
    Select TimeCreated,Id,ProviderName,LevelDisplayName,
           @{n='Msg';e={$_.Message -split "`n" | Select -First 1}} |
    Format-Table -AutoSize | Out-String -Width 200
"=== Kernel-Power / sleep-wake tonight ==="
Get-WinEvent -FilterHashtable @{LogName='System'; ProviderName='Microsoft-Windows-Kernel-Power'} -MaxEvents 8 -ErrorAction SilentlyContinue |
    Select TimeCreated,Id,@{n='Msg';e={$_.Message -split "`n" | Select -First 1}} | Format-List
"=== DriverFrameworks / device events 02:30-03:10 ==="
Get-WinEvent -FilterHashtable @{LogName='Microsoft-Windows-Kernel-PnP/Device Management'; StartTime=$start; EndTime=$end} -ErrorAction SilentlyContinue |
    Select TimeCreated,Id,@{n='Msg';e={$_.Message -split "`n" | Select -First 1}} |
    Format-Table -AutoSize | Out-String -Width 200

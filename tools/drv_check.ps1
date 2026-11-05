Get-CimInstance Win32_SystemDriver -Filter "Name like 'HWiNFO%'" |
    Select Name,State,PathName | Format-List
$drv = (Get-CimInstance Win32_SystemDriver -Filter "Name='HWiNFO_214'").PathName
$drv = $drv -replace '^\\\?\?\\',''
if ($drv) {
    (Get-Item $drv).VersionInfo |
        Select FileVersion,ProductVersion,CompanyName | Format-List
    (Get-Item $drv) | Select LastWriteTime | Format-List
}
"--- HWiNFO64.dll (vendor) ---"
(Get-Item "E:\Project\Ocypus Display rust\vendor\SystemInfos\HWiNFO64.dll").VersionInfo |
    Select FileVersion,ProductVersion | Format-List
(Get-Item "E:\Project\Ocypus Display rust\vendor\SystemInfos\HWiNFO64.dll") |
    Select LastWriteTime | Format-List
"--- HWiNFO64.dll (official Release dir) ---"
$off = "C:\Program Files\Ocypus Display\resources\main\SDK\VC#\SystemInfos\vs2008\bin\x64\Release\HWiNFO64.dll"
if (Test-Path $off) {
    (Get-Item $off).VersionInfo | Select FileVersion,ProductVersion | Format-List
}
"--- modules loaded in parked probe ---"
Get-Process SensorProbe -ErrorAction SilentlyContinue |
    Select -Expand Modules |
    Where-Object { $_.ModuleName -match 'hwinfo|HWiNFO' } |
    Select ModuleName,FileVersion | Format-Table -AutoSize

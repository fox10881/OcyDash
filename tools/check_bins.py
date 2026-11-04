import struct, json, time, glob, os, subprocess

def read(p):
    try:
        d = open(p, 'rb').read(65536)
        n = struct.unpack('<I', d[:4])[0]
        j = json.loads(d[4:4 + n])
        return j
    except Exception:
        return None

def probe(p):
    j = read(p)
    if not j:
        return None, None, None
    temps = j.get('cpuLayout', {}).get('temperature_list', [])
    loads = j.get('cpuLayout', {}).get('utilization_list', [])
    t = next((e['value'] for e in temps if 'Tctl' in e.get('sensorname', '')), None)
    l = next((e['value'] for e in loads if 'Total' in e.get('sensorname', '')), None)
    return j.get('systemTime'), t, l

roots = [
    os.path.expandvars(r'%USERPROFILE%\AppData\Local\Temp'),
    os.path.expandvars(r'%LOCALAPPDATA%\Temp'),
    os.path.expandvars(r'%TEMP%'),
    r'C:\Windows\Temp',
]
files = []
for r in dict.fromkeys(roots):
    files += glob.glob(os.path.join(r, 'Ocypus*.bin'))

print(f'{len(files)} bin files found across {len(set(roots))} roots')
for round_ in range(6):
    for p in files:
        st_, temp, load = probe(p)
        print(f'  [{round_}] {os.path.basename(p):22} sysTime={st_}  cpu_tctl={temp}  cpu_load={load}')
    if round_ < 5:
        time.sleep(3)

print('\n--- SystemInfos.exe processes ---')
try:
    out = subprocess.check_output(
        ['powershell', '-NoProfile', '-Command',
         "Get-CimInstance Win32_Process -Filter \"Name='SystemInfos.exe'\" | "
         "Select-Object ProcessId,ParentProcessId | Format-Table -AutoSize"],
        text=True, timeout=15)
    print(out)
except Exception as e:
    print('query failed:', e)

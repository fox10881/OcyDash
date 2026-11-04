import struct, json, time, glob, os, subprocess

def st(p):
    try:
        d = open(p, 'rb').read(65536)
        n = struct.unpack('<I', d[:4])[0]
        return json.loads(d[4:4 + n]).get('systemTime')
    except Exception:
        return None

roots = [
    os.path.expandvars(r'%USERPROFILE%\AppData\Local\Temp'),
    os.path.expandvars(r'%LOCALAPPDATA%\Temp'),
    os.path.expandvars(r'%TEMP%'),
    r'C:\Windows\Temp',
]
files = []
for r in dict.fromkeys(roots):
    files += glob.glob(os.path.join(r, 'Ocypus*.bin'))

t1 = {p: st(p) for p in files}
time.sleep(4)
for p in files:
    t2 = st(p)
    name = os.path.basename(p)
    state = 'LIVE' if t1[p] and t2 and t2 != t1[p] else 'dead'
    print(f'{name:24} {t1[p]} -> {t2}  {state}')

# who is SystemInfos.exe?
try:
    out = subprocess.check_output(
        ['powershell', '-NoProfile', '-Command',
         "Get-CimInstance Win32_Process -Filter \"Name='SystemInfos.exe'\" | "
         "Select-Object ProcessId,ParentProcessId,ExecutablePath,CommandLine | Format-List"],
        text=True, timeout=15)
    print(out)
except Exception as e:
    print('process query failed:', e)

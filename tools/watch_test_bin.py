import struct, json, time

p = r'C:\Users\yahya\AppData\Local\Temp\OcypusTest.bin'

def probe():
    try:
        d = open(p, 'rb').read(65536)
        n = struct.unpack('<I', d[:4])[0]
        j = json.loads(d[4:4 + n])
        temps = j.get('cpuLayout', {}).get('temperature_list', [])
        loads = j.get('cpuLayout', {}).get('utilization_list', [])
        t = next((e['value'] for e in temps if 'Tctl' in e.get('sensorname', '')), None)
        l = next((e['value'] for e in loads if 'Total' in e.get('sensorname', '')), None)
        return j.get('systemTime'), t, l
    except Exception:
        return None, None, None

for i in range(20):
    st_, t, l = probe()
    print(f'[{i*5:3}s] sysTime={st_}  cpu_tctl={t}  cpu_load={l}')
    time.sleep(5)

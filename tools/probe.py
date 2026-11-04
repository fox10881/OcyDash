import serial, json, sys, time

PORT = sys.argv[1] if len(sys.argv) > 1 else 'COM3'
BAUD = 2_000_000

def cmd_frame(key: int, value: bytes = b'') -> bytes:
    length = len(value) + 7
    frame = bytes([0x55, 0xAA, length & 0xFF, (length >> 8) & 0xFF, key]) + value
    cksum = sum(frame) & 0xFFFF
    return frame + bytes([cksum & 0xFF, (cksum >> 8) & 0xFF])

def read_frame(ser, timeout=3.0):
    """Read a 55 AA framed response."""
    buf = bytearray()
    t0 = time.time()
    while time.time() - t0 < timeout:
        b = ser.read(1)
        if not b:
            continue
        buf += b
        if len(buf) >= 2 and buf[0] == 0x55 and buf[1] == 0xAA and len(buf) >= 4:
            length = buf[2] | (buf[3] << 8)
            if len(buf) >= length:
                return bytes(buf[:length]), bytes(buf[length:])
        if len(buf) > 4096:
            break
    return bytes(buf), b''

ser = serial.Serial(PORT, BAUD, bytesize=8, parity='N', stopbits=1,
                    timeout=0.05, dsrdtr=True, rtscts=False, xonxoff=False)
ser.setDTR(True)
ser.reset_input_buffer()

print('>>> getDeviceInfo (key 0x06)')
ser.write(cmd_frame(0x06))
ser.flush()
time.sleep(0.5)

raw, extra = read_frame(ser, timeout=3.0)
print('resp hex:', raw.hex(' '))
print('trailing:', extra.hex(' '))
if len(raw) >= 7:
    payload = raw[5:-2]
    try:
        info = json.loads(payload.decode('utf-8'))
        print('JSON:', json.dumps(info, indent=1))
    except Exception:
        print('payload hex:', payload.hex(' '))

ser.close()

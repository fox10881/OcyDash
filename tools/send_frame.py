import serial, json, sys, time, os

PORT = sys.argv[1] if len(sys.argv) > 1 else 'COM3'
JPG = sys.argv[2] if len(sys.argv) > 2 else r'E:\Project\Ocypus Display rust\tools\test.jpg'
BAUD = 2_000_000

def cmd_frame(key: int, value: bytes = b'') -> bytes:
    length = len(value) + 7
    frame = bytes([0x55, 0xAA, length & 0xFF, (length >> 8) & 0xFF, key]) + value
    cksum = sum(frame) & 0xFFFF
    return frame + bytes([cksum & 0xFF, (cksum >> 8) & 0xFF])

def jpeg_frame(jpeg: bytes) -> bytes:
    n = len(jpeg)
    head = bytes([n & 0xFF, (n >> 8) & 0xFF, (n >> 16) & 0xFF, (n >> 24) & 0xFF])
    cksum = (sum(head) + sum(jpeg)) & 0xFFFF
    return head + jpeg + bytes([cksum & 0xFF, (cksum >> 8) & 0xFF])

ser = serial.Serial(PORT, BAUD, bytesize=8, parity='N', stopbits=1,
                    timeout=0.05, dsrdtr=True, rtscts=False, xonxoff=False)
ser.setDTR(True)
ser.reset_input_buffer()

jpeg = open(JPG, 'rb').read()
print('jpeg bytes:', len(jpeg), 'soi:', jpeg[:2].hex(), 'eoi:', jpeg[-2:].hex())

# enter live mode
print('>>> key 0x11 (live mode)')
ser.write(cmd_frame(0x11))
ser.flush()
time.sleep(0.2)

# stream the frame a few times so it persists
frame = jpeg_frame(jpeg)
print('>>> frame: total', len(frame), 'bytes')
for i in range(10):
    ser.write(frame)
    ser.flush()
    time.sleep(0.05)

# drain any responses
time.sleep(0.3)
resp = ser.read(4096)
print('device resp:', resp.hex(' ') if resp else '(none)')

# keepalive
ser.write(cmd_frame(0x11))
ser.flush()
ser.close()
print('done - check the cooler screen')

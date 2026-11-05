import struct, sys
f = open(r'E:\Project\Ocypus Display rust\vendor\SystemInfos\HWiNFO64.dll', 'rb').read()
pe = struct.unpack_from('<I', f, 0x3c)[0]
magic = struct.unpack_from('<H', f, pe + 24)[0]
dd_off = 0x88 if magic == 0x20B else 0x78
dd = struct.unpack_from('<I', f, pe + dd_off)[0]
num_sec = struct.unpack_from('<H', f, pe + 6)[0]
opt_size = struct.unpack_from('<H', f, pe + 20)[0]
sec_off = pe + 24 + opt_size
secs = []
for i in range(num_sec):
    b = sec_off + 40 * i
    vs, va, rs, rp = struct.unpack_from('<IIII', f, b + 8)
    secs.append((va, vs, rp))
def r2o(rva):
    for va, vs, rp in secs:
        if va <= rva < va + vs:
            return rp + rva - va
    return None
o = r2o(dd)
cnt = struct.unpack_from('<I', f, o + 20)[0]
names_rva = struct.unpack_from('<I', f, o + 28)[0]
funcs_rva = struct.unpack_from('<I', f, o + 32)[0]
ordinals_rva = struct.unpack_from('<I', f, o + 36)[0]
no = r2o(names_rva)
fo = r2o(funcs_rva)
print('exports:', cnt)
names = {}
for i in range(cnt):
    r = struct.unpack_from('<I', f, no + 4 * i)[0]
    nm = f[r2o(r):r2o(r) + 80].split(b'\0')[0].decode(errors='replace')
    print(' ', nm)

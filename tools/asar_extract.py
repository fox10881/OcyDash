import struct, json, os, sys

def parse_asar(path):
    with open(path, 'rb') as f:
        data = f.read()
    pickle_sz = struct.unpack('<I', data[0:4])[0]
    header_size = struct.unpack('<I', data[4:8])[0]
    json_len = struct.unpack('<I', data[12:16])[0]
    header = json.loads(data[16:16 + json_len])
    base_offset = 8 + header_size
    return header, base_offset, data

def walk(node, prefix, out):
    for name, entry in node.get('files', {}).items():
        p = prefix + '/' + name
        if 'files' in entry:
            walk(entry, p, out)
        else:
            out.append((p, int(entry.get('offset', 0)), entry.get('size', 0)))

if __name__ == '__main__':
    asar = sys.argv[1]
    header, base, data = parse_asar(asar)
    files = []
    walk(header, '', files)
    if len(sys.argv) > 2 and sys.argv[2] == '--list':
        for p, off, sz in files:
            print(f"{sz:>12} {p}")
        sys.exit(0)
    dest = sys.argv[2] if len(sys.argv) > 2 else 'asar_out'
    for p, off, sz in files:
        target = os.path.join(dest, p.lstrip('/'))
        os.makedirs(os.path.dirname(target), exist_ok=True)
        with open(target, 'wb') as out:
            out.write(data[base + off: base + off + sz])
    print(f"extracted {len(files)} files to {dest}")

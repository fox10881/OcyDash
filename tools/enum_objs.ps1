$src = @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public class ObjDir {
    [DllImport("ntdll.dll")] static extern int NtQueryDirectoryObject(
        IntPtr h, IntPtr buf, int len, bool single, bool restart,
        ref int ctx, out int retLen);
    [DllImport("ntdll.dll")] static extern int NtOpenDirectoryObject(
        out IntPtr h, uint access, ref OBJ_ATTR attr);
    [StructLayout(LayoutKind.Sequential)]
    struct UNICODE_STRING { public ushort Len, Max; public IntPtr Buf; }
    [StructLayout(LayoutKind.Sequential)]
    struct OBJ_ATTR { public int Length; public IntPtr Root, Name;
        public uint Attributes, SecDesc, SecQos; }
    public static void List(string dir) {
        UNICODE_STRING us = new UNICODE_STRING();
        us.Buf = Marshal.StringToHGlobalUni(dir);
        us.Len = (ushort)(dir.Length * 2); us.Max = us.Len;
        OBJ_ATTR oa = new OBJ_ATTR();
        oa.Length = Marshal.SizeOf(oa); oa.Name = Marshal.AllocHGlobal(
            Marshal.SizeOf(us));
        Marshal.StructureToPtr(us, oa.Name, true); oa.Attributes = 0x40;
        IntPtr h;
        if (NtOpenDirectoryObject(out h, 1, ref oa) != 0) {
            Console.WriteLine("cannot open " + dir); return; }
        IntPtr buf = Marshal.AllocHGlobal(65536);
        int ctx = 0;
        while (true) {
            int rl;
            int st = NtQueryDirectoryObject(h, buf, 65536, false,
                ctx == 0, ref ctx, out rl);
            if (st < 0) break;
            int off = 0;
            while (off < rl) {
                IntPtr namePtr = Marshal.ReadIntPtr(buf, off + 8);
                short nlen = Marshal.ReadInt16(buf, off + 8);
                IntPtr typePtr = Marshal.ReadIntPtr(buf, off + 16 + 8);
                short tlen = Marshal.ReadInt16(buf, off + 16 + 8);
                string n = Marshal.PtrToStringUni(namePtr, nlen / 2);
                string t = Marshal.PtrToStringUni(typePtr, tlen / 2);
                if (!string.IsNullOrEmpty(n))
                    Console.WriteLine("{0,-14} {1}", t, n);
                off += 32;
            }
            if (st != 0) break;
        }
    }
}
'@
Add-Type -TypeDefinition $src -Language CSharp
"=== \BaseNamedObjects (HWiNFO/Ocypus/Sensor hits) ==="
[ObjDir]::List("\BaseNamedObjects") |
    Where-Object { $_ -match 'hwinfo|ocypus|sensor|systeminfo' }
"=== \Sessions\1\BaseNamedObjects (HWiNFO/Ocypus hits) ==="
[ObjDir]::List("\Sessions\1\BaseNamedObjects") |
    Where-Object { $_ -match 'hwinfo|ocypus|sensor|systeminfo' }

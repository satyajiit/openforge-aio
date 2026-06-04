#!/usr/bin/env python3
"""External-RPM UObject vtable dumper for the no-hardcode ProcessEvent hunt.

Walks GUObjectArray in the live game (read-only ReadProcessMemory), pulls the
vtable of the first live UObject + a handful of *different-class* objects, and
prints each vtable slot's function pointer (as a module RVA). The slot whose
pointer is identical across many classes AND lands in .text is a UObject base
virtual; cross-referenced with an on-disk prologue scan that's how we pin
ProcessEvent without any baked absolute address.

Usage: python rpm_vtable.py <pid> <guobject_array_va_hex> [--base 0x140000000] [--slots 96]
"""
import ctypes as C
import struct
import sys
from ctypes import wintypes

k32 = C.WinDLL("kernel32", use_last_error=True)
PROCESS_VM_READ = 0x0010
PROCESS_QUERY_INFORMATION = 0x0400

k32.OpenProcess.restype = wintypes.HANDLE
k32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
k32.ReadProcessMemory.restype = wintypes.BOOL
k32.ReadProcessMemory.argtypes = [
    wintypes.HANDLE, wintypes.LPCVOID, wintypes.LPVOID, C.c_size_t,
    C.POINTER(C.c_size_t),
]

CHUNK_SIZE = 65536
STRIDE = 24
NAME_OFF = 0x18      # UObject::NamePrivate (FName)
CLASS_OFF = 0x10     # UObject::ClassPrivate


class RPM:
    def __init__(self, pid):
        self.h = k32.OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION, False, pid)
        if not self.h:
            raise OSError(f"OpenProcess({pid}) failed: {C.get_last_error()}")

    def read(self, addr, n):
        buf = (C.c_char * n)()
        got = C.c_size_t(0)
        ok = k32.ReadProcessMemory(self.h, C.c_void_p(addr), buf, n, C.byref(got))
        if not ok:
            return None
        return bytes(buf[: got.value])

    def u64(self, addr):
        b = self.read(addr, 8)
        return struct.unpack("<Q", b)[0] if b and len(b) == 8 else None

    def u32(self, addr):
        b = self.read(addr, 4)
        return struct.unpack("<I", b)[0] if b and len(b) == 4 else None


def parse_sections(data):
    e_lfanew = struct.unpack_from("<I", data, 0x3C)[0]
    num_sections = struct.unpack_from("<H", data, e_lfanew + 6)[0]
    size_opt = struct.unpack_from("<H", data, e_lfanew + 20)[0]
    sect_tab = e_lfanew + 24 + size_opt
    out = []
    for i in range(num_sections):
        off = sect_tab + i * 40
        vsize, vaddr, rawsize, rawptr = struct.unpack_from("<IIII", data, off + 8)
        out.append((vaddr, vsize, rawptr, rawsize))
    return out


def rva_prologue(data, sections, rva, n=32):
    for vaddr, vsize, rawptr, rawsize in sections:
        if vaddr <= rva < vaddr + max(vsize, rawsize):
            o = rawptr + (rva - vaddr)
            return data[o:o + n]
    return b""


def main():
    args = sys.argv[1:]
    base = 0x140000000
    slots = 96
    exe = None
    if "--base" in args:
        i = args.index("--base"); base = int(args[i + 1], 16); del args[i:i + 2]
    if "--slots" in args:
        i = args.index("--slots"); slots = int(args[i + 1]); del args[i:i + 2]
    if "--exe" in args:
        i = args.index("--exe"); exe = args[i + 1]; del args[i:i + 2]
    pid = int(args[0]); guo = int(args[1], 16)
    rpm = RPM(pid)

    hdr = rpm.read(guo, 32)
    if not hdr:
        print(f"FAILED to read GUObjectArray header @ 0x{guo:X}"); return
    objects_ptr = struct.unpack_from("<Q", hdr, 0)[0]
    num_elements = struct.unpack_from("<I", hdr, 0x14)[0]
    num_chunks = struct.unpack_from("<I", hdr, 0x1C)[0]
    print(f"GUObjectArray @0x{guo:X}: objects_ptr=0x{objects_ptr:X} num_elements={num_elements} num_chunks={num_chunks}")
    if not (objects_ptr and 0 < num_elements < 50_000_000):
        print("header invalid — wrong GUObjectArray VA"); return

    def obj_at(index):
        ci, wi = index // CHUNK_SIZE, index % CHUNK_SIZE
        chunk = rpm.u64(objects_ptr + ci * 8)
        if not chunk:
            return None
        return rpm.u64(chunk + wi * STRIDE)

    # Collect objects with DISTINCT vtables (overriding classes). Universal-slot
    # detection only works when the sampled vtables actually differ — many BP
    # classes share the engine vtable, so we dedup on the vtable pointer and
    # scan a wide, spread-out range of the object array.
    vtables = []        # (obj, cls, vtable_va)
    seen_vt = set()
    step = max(1, num_elements // 40000)
    for idx in range(0, num_elements, step):
        obj = obj_at(idx)
        if not obj or obj % 8 or obj <= 0x10000:
            continue
        vt = rpm.u64(obj)
        if not vt or not (base <= vt < base + 0x10000000):
            continue
        if vt in seen_vt:
            continue
        seen_vt.add(vt)
        cls = rpm.u64(obj + CLASS_OFF)
        vtables.append((obj, cls, vt))
        if len(vtables) >= 16:
            break

    print(f"sampled {len(vtables)} distinct-class objects")
    # Read each vtable's slots.
    slotvals = []  # list per object: [rva or None]
    for obj, cls, vt in vtables:
        row = []
        raw = rpm.read(vt, slots * 8)
        for s in range(slots):
            v = struct.unpack_from("<Q", raw, s * 8)[0] if raw and (s + 1) * 8 <= len(raw) else 0
            row.append(v)
        slotvals.append(row)

    # A "universal" slot = same pointer across >=80% of sampled objects, in-module.
    # ProcessEvent is a UObject base virtual present at one such slot. We then
    # tally how many slots each universal RVA occupies: tiny shared stubs
    # (pure-virtual asserts, `mov al,1;ret`) fill MANY slots; ProcessEvent
    # occupies exactly ONE. Reading each candidate's on-disk prologue lets us
    # pick ProcessEvent by its large stack frame.
    from collections import Counter
    univ = {}  # slot_index -> rva
    for s in range(slots):
        vals = [row[s] for row in slotvals]
        c = Counter(vals)
        top, cnt = c.most_common(1)[0]
        if cnt >= max(2, (len(vals) * 4) // 5) and base <= top < base + 0x10000000:
            univ[s] = top - base
    occ = Counter(univ.values())  # how many slots each RVA fills

    data = sections = None
    if exe:
        with open(exe, "rb") as f:
            data = f.read()
        sections = parse_sections(data)

    print(f"\n{len(univ)} universal slots; "
          f"{sum(1 for v in occ.values() if v == 1)} occupy a single slot (PE candidates)\n")
    print("idx  | rva        | #slots | prologue (on-disk)")
    for s in sorted(univ):
        rva = univ[s]
        n_slots = occ[rva]
        if n_slots > 2:
            continue  # shared stub, not ProcessEvent
        pro = rva_prologue(data, sections, rva, 28) if data else b""
        prohex = " ".join(f"{b:02X}" for b in pro)
        print(f"{s:4d} | 0x{rva:08X} | {n_slots:5d}  | {prohex}")


if __name__ == "__main__":
    main()

"""Minimal x64 minidump faulting-thread stack scanner.

No symbols required. Parses the stream directory, reads the ExceptionStream
(faulting thread + exception address/code), the ModuleListStream (to map
addresses -> module+offset), and the faulting thread's stack memory. It then
scans the stack for 8-byte values that land inside any module's image range
(return-address candidates) and prints them as module+offset in call order.

Decisive question for the LotDK crash: does `batman_lod_dll.dll` appear on the
faulting thread's stack? If yes, our injected code is in the crash chain; if
no, the game's own thread faulted with us merely attached.
"""

import struct
import sys

EXCEPTION_STREAM = 6
MODULE_LIST_STREAM = 4
THREAD_LIST_STREAM = 3


def u16(b, o):
    return struct.unpack_from("<H", b, o)[0]


def u32(b, o):
    return struct.unpack_from("<I", b, o)[0]


def u64(b, o):
    return struct.unpack_from("<Q", b, o)[0]


def read_mdstring(b, rva):
    n = u32(b, rva)  # length in BYTES of the UTF-16 string
    raw = b[rva + 4 : rva + 4 + n]
    return raw.decode("utf-16-le", errors="replace")


def main(path):
    with open(path, "rb") as f:
        b = f.read()
    assert b[:4] == b"MDMP", "not a minidump"
    nstreams = u32(b, 8)
    dir_rva = u32(b, 12)

    streams = {}
    for i in range(nstreams):
        e = dir_rva + i * 12
        stype = u32(b, e)
        dsize = u32(b, e + 4)
        rva = u32(b, e + 8)
        streams[stype] = (dsize, rva)

    # --- modules ---
    mods = []  # (base, size, name)
    if MODULE_LIST_STREAM in streams:
        _, rva = streams[MODULE_LIST_STREAM]
        nmod = u32(b, rva)
        p = rva + 4
        for _ in range(nmod):
            base = u64(b, p)
            size = u32(b, p + 8)
            name_rva = u32(b, p + 20)
            name = read_mdstring(b, name_rva)
            mods.append((base, size, name.split("\\")[-1]))
            p += 108
    mods.sort()

    def mod_of(addr):
        for base, size, name in mods:
            if base <= addr < base + size:
                return name, addr - base
        return None, None

    # --- exception ---
    fault_tid = None
    if EXCEPTION_STREAM in streams:
        _, rva = streams[EXCEPTION_STREAM]
        fault_tid = u32(b, rva)
        code = u32(b, rva + 8)
        exc_addr = u64(b, rva + 24)
        m, off = mod_of(exc_addr)
        print(f"== exception ==")
        print(f"  code        = 0x{code:08X}")
        print(f"  fault thread= {fault_tid} (0x{fault_tid:X})")
        loc = f"{m}+0x{off:X}" if m else "??"
        print(f"  fault addr  = 0x{exc_addr:X}  ({loc})")
    print()

    dll_base = dll_size = None
    for base, size, name in mods:
        if name.lower() == "batman_lod_dll.dll":
            dll_base, dll_size = base, size
        if name.lower().startswith("legobatman"):
            print(f"  game module : {name} base=0x{base:X} size=0x{size:X}")
    if dll_base:
        print(f"  our DLL     : batman_lod_dll.dll base=0x{dll_base:X} size=0x{dll_size:X}")
    else:
        print("  our DLL     : batman_lod_dll.dll NOT in module list")
    print()

    # --- faulting thread's stack ---
    if THREAD_LIST_STREAM not in streams:
        print("no thread list stream")
        return
    _, rva = streams[THREAD_LIST_STREAM]
    nthreads = u32(b, rva)
    p = rva + 4
    target = None
    for _ in range(nthreads):
        tid = u32(b, p)
        stack_start = u64(b, p + 24)
        stack_dsize = u32(b, p + 32)
        stack_rva = u32(b, p + 36)
        if tid == fault_tid:
            target = (stack_start, stack_dsize, stack_rva)
            break
        p += 48
    if not target:
        print("faulting thread not found in thread list")
        return
    stack_start, dsize, srva = target
    print(f"== faulting thread stack ==  VA[0x{stack_start:X}..0x{stack_start+dsize:X}] size=0x{dsize:X}")
    stack = b[srva : srva + dsize]

    # Scan 8-byte-aligned values; report ones inside any module image (likely
    # return addresses). Print in call order (low VA = innermost frame first).
    print("  return-address candidates (module+offset), innermost first:")
    dll_hits = 0
    shown = 0
    for off in range(0, len(stack) - 8, 8):
        v = u64(stack, off)
        m, moff = mod_of(v)
        if m is None:
            continue
        is_dll = dll_base is not None and dll_base <= v < dll_base + dll_size
        if is_dll:
            dll_hits += 1
        # Show DLL hits always; show game/other frames up to a cap to keep it readable.
        if is_dll or shown < 60:
            tag = "  <-- batman_lod_dll.dll" if is_dll else ""
            va = stack_start + off
            print(f"    [sp+0x{off:05X}] 0x{v:X}  {m}+0x{moff:X}{tag}")
            shown += 1
    print()
    print(f"== VERDICT: batman_lod_dll.dll appears {dll_hits} time(s) on the faulting thread's stack ==")
    if dll_hits == 0:
        print("  => our DLL is NOT in the crash chain; the GAME's own thread faulted.")
    else:
        print("  => our injected code IS in the crash chain.")


if __name__ == "__main__":
    main(sys.argv[1])

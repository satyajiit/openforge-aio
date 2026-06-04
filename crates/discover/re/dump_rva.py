#!/usr/bin/env python3
"""Dump N bytes at one or more module RVAs from an on-disk PE image.

Used to extract function-prologue *code signatures* from a known-good build:
we read the bytes at a known RVA, then synthesize a wildcarded AOB pattern so
the in-DLL scanner can re-find the function on any relinked build (game patch
/ store variant) WITHOUT a baked absolute address.

Usage:
    python dump_rva.py <pe_path> <rva_hex> [<rva_hex> ...] [--len N]
RVAs are module-relative (image_base subtracted), e.g. ProcessEvent at
0x14AB884 for an image based at 0x140000000.
"""
import struct
import sys


def parse_sections(data):
    e_lfanew = struct.unpack_from("<I", data, 0x3C)[0]
    # COFF header
    num_sections = struct.unpack_from("<H", data, e_lfanew + 6)[0]
    size_opt = struct.unpack_from("<H", data, e_lfanew + 20)[0]
    sect_tab = e_lfanew + 24 + size_opt
    sections = []
    for i in range(num_sections):
        off = sect_tab + i * 40
        name = data[off : off + 8].rstrip(b"\x00").decode("latin1")
        vsize, vaddr, rawsize, rawptr = struct.unpack_from("<IIII", data, off + 8)
        sections.append((name, vaddr, vsize, rawptr, rawsize))
    return sections


def rva_to_off(sections, rva):
    for name, vaddr, vsize, rawptr, rawsize in sections:
        if vaddr <= rva < vaddr + max(vsize, rawsize):
            return rawptr + (rva - vaddr)
    raise ValueError(f"RVA 0x{rva:X} not in any section")


def main():
    args = sys.argv[1:]
    length = 64
    if "--len" in args:
        i = args.index("--len")
        length = int(args[i + 1])
        del args[i : i + 2]
    pe_path, rvas = args[0], [int(x, 16) for x in args[1:]]
    with open(pe_path, "rb") as f:
        data = f.read()
    sections = parse_sections(data)
    for rva in rvas:
        off = rva_to_off(sections, rva)
        chunk = data[off : off + length]
        hexbytes = " ".join(f"{b:02X}" for b in chunk)
        print(f"RVA 0x{rva:X}  (file_off 0x{off:X})  {length} bytes:")
        print(f"  {hexbytes}")
        print()


if __name__ == "__main__":
    main()

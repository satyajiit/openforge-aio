#!/usr/bin/env python3
"""Scan an on-disk PE's executable sections for a wildcarded AOB pattern.

Used to validate function-prologue *code signatures* before baking them into
the DLL's no-hardcode discovery: confirm the pattern matches the known target
and report how many total hits it has across .text (uniqueness check).

Usage: python scan_aob.py <pe_path> "<AOB with ?? wildcards>"
"""
import struct
import sys


def parse_sections(data):
    e_lfanew = struct.unpack_from("<I", data, 0x3C)[0]
    num_sections = struct.unpack_from("<H", data, e_lfanew + 6)[0]
    size_opt = struct.unpack_from("<H", data, e_lfanew + 20)[0]
    sect_tab = e_lfanew + 24 + size_opt
    out = []
    for i in range(num_sections):
        off = sect_tab + i * 40
        vsize, vaddr, rawsize, rawptr = struct.unpack_from("<IIII", data, off + 8)
        charac = struct.unpack_from("<I", data, off + 36)[0]
        out.append((vaddr, vsize, rawptr, rawsize, charac))
    return out


def parse_pattern(p):
    toks = p.split()
    pat, mask = [], []
    for t in toks:
        if t == "??":
            pat.append(0); mask.append(False)
        else:
            pat.append(int(t, 16)); mask.append(True)
    return pat, mask


def main():
    pe_path, pat_str = sys.argv[1], sys.argv[2]
    with open(pe_path, "rb") as f:
        data = f.read()
    pat, mask = parse_pattern(pat_str)
    n = len(pat)
    hits = []
    for vaddr, vsize, rawptr, rawsize, charac in parse_sections(data):
        if not (charac & 0x20000020):  # executable / code
            continue
        seg = data[rawptr:rawptr + rawsize]
        for i in range(len(seg) - n + 1):
            ok = True
            for j in range(n):
                if mask[j] and seg[i + j] != pat[j]:
                    ok = False
                    break
            if ok:
                hits.append(vaddr + i)
    print(f"pattern ({n} bytes) matched {len(hits)} time(s):")
    for h in hits[:20]:
        print(f"  RVA 0x{h:X}")


if __name__ == "__main__":
    main()

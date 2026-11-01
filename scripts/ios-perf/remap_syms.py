#!/usr/bin/env python3
"""Names a new trace's bare addresses from an earlier atos run, by (image path, offset from the
image's load address). Saves a second symbolication when the binaries did not change.
Usage: remap_syms.py old_addrs.tsv old_syms.tsv new_addrs.tsv out_syms.tsv
(the .addrs.tsv and .syms.tsv files are what sim_symbol_addrs.py and sim-symbolicate.sh write)"""
import sys


def main():
    old_addrs, old_syms, new_addrs, out = sys.argv[1:5]
    sym_of = {}
    for line in open(old_syms):
        a, s = line.rstrip("\n").split("\t", 1)
        sym_of[a] = s
    by_off = {}
    for line in open(old_addrs):
        path, load, addr = line.rstrip("\n").split("\t")
        if addr in sym_of and not sym_of[addr].startswith("0x"):
            by_off[(path, int(addr, 16) - int(load, 16))] = sym_of[addr]
    hit = miss = 0
    with open(out, "w") as f:
        for line in open(new_addrs):
            path, load, addr = line.rstrip("\n").split("\t")
            s = by_off.get((path, int(addr, 16) - int(load, 16)))
            if s:
                f.write(f"{addr}\t{s}\n")
                hit += 1
            else:
                miss += 1
    print(f"named {hit}, unnamed {miss} ({100 * hit / max(1, hit + miss):.1f}%)")


if __name__ == "__main__":
    main()

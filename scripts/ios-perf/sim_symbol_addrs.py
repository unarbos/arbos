"""List the unsymbolised frames of an `xctrace export` time-profile XML for sim-symbolicate.sh.
Usage: sim_symbol_addrs.py <name>.tp.xml addrs.tsv  (appends path<TAB>load-addr<TAB>addr)."""
import re, sys, collections
s = open(sys.argv[1]).read()
bins = {}
for m in re.finditer(r'<binary id="(\d+)" name="([^"]*)"[^>]*load-addr="([^"]*)" path="([^"]*)"', s):
    bins[m.group(1)] = (m.group(2), m.group(3), m.group(4))
out = collections.defaultdict(set)
for m in re.finditer(r'<frame id="\d+" name="(0x[0-9a-f]+)" addr="(0x[0-9a-f]+)"\s*>\s*<binary (?:id|ref)="(\d+)"', s):
    b = bins.get(m.group(3))
    if b:
        out[(b[1], b[2])].add(m.group(2))
with open(sys.argv[2], "a") as f:
    for (load, path), addrs in out.items():
        for a in sorted(addrs):
            f.write(f"{path}\t{load}\t{a}\n")
print(sum(len(v) for v in out.values()), len(out))

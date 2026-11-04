#!/usr/bin/env python3
"""Put the SIL Open Font License back into the bundled Inter subsets' `name` table.

`crates/arbos-core/assets/Inter-{Regular,SemiBold}-subset.ttf` are 205-codepoint cuts of
the Inter 4.1 release (`extras/ttf/Inter-Regular.ttf`, `extras/ttf/Inter-SemiBold.ttf`;
internal version `4.001;git-9221beed3`). They were made with `pyftsubset`, whose default
`--name-IDs=0,1,2,3,4,5,6` keeps the copyright string and drops name ID 13 (license
description) and name ID 14 (license URL). OFL 1.1 clause 2 wants the copyright notice
*and* the license to travel with every copy, so dropping 13 and 14 leaves a shipped
binary that names its copyright holder and not its license.

This tool writes those two records back. Nothing else about the font changes: every other
sfnt table is copied byte for byte, and the tool refuses to write if that is not true of
its own output. Rendering, metrics and glyph coverage are therefore untouched.

    scripts/fonts/inter-license-names.py --check                 # are 13 and 14 present?
    scripts/fonts/inter-license-names.py --write                 # put them back
    scripts/fonts/inter-license-names.py --verify-against DIR    # prove the strings are upstream's

`--verify-against` takes a directory holding upstream `Inter-Regular.ttf` and
`Inter-SemiBold.ttf` (the `extras/ttf/` folder of the Inter 4.1 release zip) and checks
both that the two strings below are copied from it and that name IDs 0-6 of the subsets
still match it, so the provenance claim is checkable and not just asserted.

A future re-subset must pass `--name-IDs+=13,14` to `pyftsubset`, or run this tool after.
"""

from __future__ import annotations

import argparse
import struct
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
ASSETS = ROOT / "crates" / "arbos-core" / "assets"
SUBSETS = ("Inter-Regular-subset.ttf", "Inter-SemiBold-subset.ttf")
UPSTREAM = {
    "Inter-Regular-subset.ttf": "Inter-Regular.ttf",
    "Inter-SemiBold-subset.ttf": "Inter-SemiBold.ttf",
}

# Name IDs 13 and 14, verbatim from the Inter 4.1 release the subsets were cut from. The
# same two strings appear in every static face of that release.
LICENSE_DESCRIPTION = (
    "This Font Software is licensed under the SIL Open Font License, Version 1.1. "
    "This license is available with a FAQ at: http://scripts.sil.org/OFL"
)
LICENSE_URL = "http://scripts.sil.org/OFL"
LICENSE_RECORDS = {13: LICENSE_DESCRIPTION, 14: LICENSE_URL}

# Windows / Unicode BMP / en-US: the only (platform, encoding, language) triple the Inter
# release and these subsets use.
WINDOWS_UNICODE_EN_US = (3, 1, 0x0409)

# The records a subset must share with its upstream face for the provenance claim to hold:
# copyright, family, subfamily, unique id, full name, version, PostScript name.
PROVENANCE_IDS = (0, 1, 2, 3, 4, 5, 6)

HEAD_CHECKSUM_ADJUSTMENT_OFFSET = 8
SFNT_CHECKSUM_MAGIC = 0xB1B0AFBA


def read_sfnt(data: bytes) -> tuple[bytes, list[str], dict[str, bytes]]:
    """Split a font into (header, tag order, tag -> unpadded bytes)."""
    num_tables = struct.unpack(">H", data[4:6])[0]
    header = data[:12]
    order: list[str] = []
    tables: dict[str, bytes] = {}
    for i in range(num_tables):
        entry = 12 + 16 * i
        tag, _checksum, offset, length = struct.unpack(">4sIII", data[entry : entry + 16])
        name = tag.decode("latin-1")
        order.append(name)
        tables[name] = data[offset : offset + length]
    return header, order, tables


def parse_names(table: bytes) -> list[tuple[tuple[int, int, int, int], str]]:
    """The `name` table as [((platform, encoding, language, nameID), string)]."""
    fmt, count, string_offset = struct.unpack(">HHH", table[:6])
    if fmt != 0:
        raise SystemExit(f"name table format {fmt} is not supported (expected 0)")
    out = []
    for i in range(count):
        rec = 6 + 12 * i
        platform, encoding, language, name_id, length, offset = struct.unpack(
            ">HHHHHH", table[rec : rec + 12]
        )
        start = string_offset + offset
        raw = table[start : start + length]
        text = raw.decode("utf-16-be") if platform in (0, 3) else raw.decode("latin-1")
        out.append(((platform, encoding, language, name_id), text))
    return out


def build_name_table(records: list[tuple[tuple[int, int, int, int], str]]) -> bytes:
    """A format 0 `name` table. Records are sorted as the spec requires; equal strings share
    storage, which is what the subsetter does too."""
    records = sorted(records, key=lambda r: r[0])
    storage = bytearray()
    offsets: dict[bytes, int] = {}
    entries = bytearray()
    for (platform, encoding, language, name_id), text in records:
        raw = text.encode("utf-16-be") if platform in (0, 3) else text.encode("latin-1")
        if raw not in offsets:
            offsets[raw] = len(storage)
            storage += raw
        entries += struct.pack(
            ">HHHHHH", platform, encoding, language, name_id, len(raw), offsets[raw]
        )
    string_offset = 6 + len(entries)
    return struct.pack(">HHH", 0, len(records), string_offset) + bytes(entries) + bytes(storage)


def table_checksum(padded: bytes) -> int:
    total = 0
    for i in range(0, len(padded), 4):
        total += struct.unpack(">I", padded[i : i + 4])[0]
    return total & 0xFFFFFFFF


def write_sfnt(header: bytes, order: list[str], tables: dict[str, bytes]) -> bytes:
    """Reassemble a font, 4-byte aligning every table and fixing all three checksums."""
    if "head" in tables:
        head = bytearray(tables["head"])
        struct.pack_into(">I", head, HEAD_CHECKSUM_ADJUSTMENT_OFFSET, 0)
        tables = {**tables, "head": bytes(head)}
    directory = bytearray()
    body = bytearray()
    base = 12 + 16 * len(order)
    for tag in order:
        raw = tables[tag]
        padded = raw + b"\0" * (-len(raw) % 4)
        directory += struct.pack(
            ">4sIII", tag.encode("latin-1"), table_checksum(padded), base + len(body), len(raw)
        )
        body += padded
    font = bytearray(header + bytes(directory) + bytes(body))
    if "head" in tables:
        head_offset = base + sum(
            len(tables[t]) + (-len(tables[t]) % 4) for t in order[: order.index("head")]
        )
        adjustment = (SFNT_CHECKSUM_MAGIC - table_checksum(bytes(font))) & 0xFFFFFFFF
        struct.pack_into(
            ">I", font, head_offset + HEAD_CHECKSUM_ADJUSTMENT_OFFSET, adjustment
        )
    return bytes(font)


def patch(data: bytes) -> bytes:
    """Return `data` with name IDs 13 and 14 present, and every other table untouched."""
    header, order, tables = read_sfnt(data)
    records = parse_names(tables["name"])
    keep = [r for r in records if r[0][3] not in LICENSE_RECORDS]
    platform, encoding, language = WINDOWS_UNICODE_EN_US
    added = [
        ((platform, encoding, language, name_id), text)
        for name_id, text in sorted(LICENSE_RECORDS.items())
    ]
    out = write_sfnt(header, order, {**tables, "name": build_name_table(keep + added)})
    # The whole point of patching rather than re-subsetting: only `name` may move.
    _, new_order, new_tables = read_sfnt(out)
    if new_order != order:
        raise SystemExit("internal error: the table order changed")
    for tag in order:
        if tag not in ("name", "head") and new_tables[tag] != tables[tag]:
            raise SystemExit(f"internal error: table {tag!r} changed")
    old_head, new_head = tables["head"], new_tables["head"]
    if old_head[:8] != new_head[:8] or old_head[12:] != new_head[12:]:
        raise SystemExit("internal error: `head` changed beyond its checksum adjustment")
    return out


def missing_license_ids(path: Path) -> list[int]:
    records = dict(parse_names(read_sfnt(path.read_bytes())[2]["name"]))
    present = {
        name_id: text
        for (_p, _e, _l, name_id), text in records.items()
        if name_id in LICENSE_RECORDS
    }
    return [
        name_id
        for name_id, text in sorted(LICENSE_RECORDS.items())
        if present.get(name_id) != text
    ]


def cmd_check() -> int:
    bad = False
    for name in SUBSETS:
        missing = missing_license_ids(ASSETS / name)
        if missing:
            bad = True
            print(f"{name}: missing or wrong name ID {', '.join(map(str, missing))}")
        else:
            print(f"{name}: name IDs 13 and 14 carry the OFL 1.1 notice")
    return 1 if bad else 0


def cmd_write() -> int:
    for name in SUBSETS:
        path = ASSETS / name
        before = path.read_bytes()
        after = patch(before)
        if after == before:
            print(f"{name}: already carries name IDs 13 and 14")
            continue
        path.write_bytes(after)
        print(f"{name}: wrote name IDs 13 and 14 ({len(before)} -> {len(after)} bytes)")
    return 0


def cmd_verify_against(directory: Path) -> int:
    failures = 0
    for name in SUBSETS:
        upstream = directory / UPSTREAM[name]
        if not upstream.is_file():
            print(f"{upstream}: not found")
            return 2
        theirs = dict(parse_names(read_sfnt(upstream.read_bytes())[2]["name"]))
        ours = dict(parse_names(read_sfnt((ASSETS / name).read_bytes())[2]["name"]))
        complaints = []
        for key, text in sorted(theirs.items()):
            name_id = key[3]
            if name_id in LICENSE_RECORDS and text != LICENSE_RECORDS[name_id]:
                complaints.append(f"name ID {name_id} in this tool is not {upstream.name}'s: {text!r}")
            if name_id in PROVENANCE_IDS and ours.get(key) != text:
                complaints.append(f"name ID {name_id} does not match {upstream.name}: {ours.get(key)!r}")
        for complaint in complaints:
            print(f"{name}: {complaint}")
        failures += len(complaints)
        if not complaints:
            print(f"{name}: name IDs 0-6 match {upstream.name}, and 13/14 are its own strings")
    return 1 if failures else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--check", action="store_true", help="report whether 13 and 14 are present")
    group.add_argument("--write", action="store_true", help="put 13 and 14 back")
    group.add_argument(
        "--verify-against",
        metavar="DIR",
        type=Path,
        help="directory with upstream Inter-Regular.ttf and Inter-SemiBold.ttf",
    )
    args = parser.parse_args()
    if args.check:
        return cmd_check()
    if args.write:
        return cmd_write()
    return cmd_verify_against(args.verify_against)


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
"""Dumps the app's URL cache (Cache.db) from a simulator: each cached URL, its time stamp, and the
decoded request and response properties. Authorization values are shown only as `<present>`.
This is the evidence for which fixture avatar URLs the app fetched at launch.
Usage: sim_url_cache.py [SIM_UDID] [OUT_DIR]   (default SIM_UDID is $SIM; reads a copy, not the live db)"""
import glob
import os
import plistlib
import shutil
import sqlite3
import sys


def walk(x, depth=0):
    pad = "    " * depth
    if isinstance(x, dict):
        for k, v in x.items():
            if isinstance(v, (dict, list)):
                print(pad + f"  {k}:")
                walk(v, depth + 1)
            else:
                print(pad + f"  {k}: {v if 'uthor' not in str(k) else '<present>'}")
    elif isinstance(x, list):
        for v in x:
            if isinstance(v, (dict, list)):
                walk(v, depth)
            elif isinstance(v, (str, int, float)) and len(str(v)) < 200:
                print(pad + f"  - {v}")


def main():
    sim = sys.argv[1] if len(sys.argv) > 1 else os.environ["SIM"]
    out = sys.argv[2] if len(sys.argv) > 2 else "/tmp/sim-url-cache"
    pattern = (f"~/Library/Developer/CoreSimulator/Devices/{sim}/data/Containers/Data/Application/"
               "*/Library/Caches/com.unarbos.arbos.ios/Cache.db")
    os.makedirs(out, exist_ok=True)
    for src in glob.glob(os.path.expanduser(pattern)):
        for suf in ("", "-wal", "-shm"):
            if os.path.exists(src + suf):
                shutil.copy(src + suf, f"{out}/Cache.db{suf}")
        db = sqlite3.connect(f"{out}/Cache.db")
        print("==", src)
        for eid, key, ts, part in db.execute(
                "select entry_ID, request_key, time_stamp, partition from cfurl_cache_response order by entry_ID"):
            print(f"entry {eid} ts={ts} partition={part}\n  url={key}")
            row = db.execute("select request_object, response_object from cfurl_cache_blob_data where entry_ID=?",
                             (eid,)).fetchone()
            for name, blob in zip(("request", "response"), row or ()):
                try:
                    p = plistlib.loads(blob)
                except Exception as e:
                    print("  ", name, "undecodable", e)
                    continue
                print("  ", name, ":")
                walk(p.get("Array", p) if isinstance(p, dict) else p)


if __name__ == "__main__":
    main()

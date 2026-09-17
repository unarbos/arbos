#!/usr/bin/env python3
"""The docs-mirror refused or failed in a cycle: record it, name what is missing, stage a restore.

    mirror-alarm.py <loop-dir> <exit-code> <mirror-script> <repo>

A refusal (exit 2) means the store view looks damaged or docs/ is gone (internal/store-docs-mirror.md).
This writes <loop>/store-mirror-history.jsonl, drafts bugs/store-docs-mirror-refused.md with the
store listing against the branch's, and — when the store's docs/ is missing or emptier than the
branch — restores the branch into <loop>/../state/store-docs-restore-<ts>/ (never into the store:
a person or the next QA turn checks the loss is real before copying anything back).
"""

import json
import os
import subprocess
import sys
import time

STORE = "/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983"


def main():
    loop, rc, script, repo = sys.argv[1], int(sys.argv[2]), sys.argv[3], sys.argv[4]
    stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    store_docs = sorted(f for f in os.listdir(f"{STORE}/docs") if f.endswith(".md")) if os.path.isdir(f"{STORE}/docs") else None
    branch_docs = []
    try:
        subprocess.run(["git", "-C", repo, "fetch", "-q", "origin", "+refs/heads/store-docs:refs/remotes/origin/store-docs"], capture_output=True, timeout=60)
        out = subprocess.run(["git", "-C", repo, "ls-tree", "--name-only", "origin/store-docs", "docs/"], capture_output=True, text=True, timeout=30).stdout
        branch_docs = sorted(os.path.basename(l) for l in out.splitlines() if l.endswith(".md"))
    except Exception as e:  # noqa: BLE001
        branch_docs = [f"(ls-tree failed: {e})"]
    missing = sorted(set(branch_docs) - set(store_docs or [])) if store_docs is not None else branch_docs
    line = {"ts": stamp, "exit": rc, "store_docs": None if store_docs is None else len(store_docs), "branch_docs": len(branch_docs), "missing_in_store": missing[:40], "notes_md": os.path.isfile(f"{STORE}/notes.md")}
    restore_dir = ""
    if rc == 3:
        line["note"] = "internal/mirror-docs.sh is gone from the store; the branch carries a copy (git show origin/store-docs:mirror-docs.sh)"
    if rc in (2, 3) and branch_docs and (store_docs is None or len(missing) > 0 or rc == 3):
        restore_dir = os.path.join(os.path.dirname(loop.rstrip("/")), "state", f"store-docs-restore-{stamp}")
        tool = script if os.path.exists(script) else "/tmp/mirror-docs-from-branch.sh"
        if not os.path.exists(script):
            with open(tool, "w") as f:
                f.write(subprocess.run(["git", "-C", repo, "show", "origin/store-docs:mirror-docs.sh"], capture_output=True, text=True, timeout=30).stdout)
        r = subprocess.run(["bash", tool, "restore", restore_dir], capture_output=True, text=True, timeout=300, env={**os.environ, "REPO": repo})
        line["restore"] = {"dir": restore_dir, "rc": r.returncode, "log": (r.stderr or r.stdout)[-200:]}
    os.makedirs(loop, exist_ok=True)
    with open(os.path.join(loop, "store-mirror-history.jsonl"), "a") as f:
        f.write(json.dumps(line) + "\n")
    print(f"-- mirror-alarm: exit {rc}; store docs {line['store_docs']}, branch docs {line['branch_docs']}, missing in store {len(missing)}" + (f"; restored to {restore_dir}" if restore_dir else ""))
    bugs = os.path.join(loop, "bugs")
    os.makedirs(bugs, exist_ok=True)
    path = os.path.join(bugs, "store-docs-mirror-refused.md")
    detail = f"exit {rc}; store docs/: {'missing' if store_docs is None else f'{len(store_docs)} files'}; branch: {len(branch_docs)} files; missing in store: {missing[:12]}; notes.md present: {line['notes_md']}" + (f"; branch restored to `{restore_dir}` (not copied into the store)" if restore_dir else "")
    if os.path.exists(path):
        with open(path, "a") as f:
            f.write(f"\n- seen again {stamp}: {detail}\n")
    else:
        with open(path, "w") as f:
            f.write(f"# store-docs-mirror-refused: the docs mirror refused or failed in a QA cycle\n\n- First seen: {stamp}\n- What it means: `internal/mirror-docs.sh` refuses to push when the store view looks damaged or `docs/` is gone (`internal/store-docs-mirror.md`). Either the store faulted again or a document was deleted. Check whether the loss is real, restore from `store-docs` (`bash internal/mirror-docs.sh restore <dir>`), tell Jacob.\n- Detail: {detail}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())

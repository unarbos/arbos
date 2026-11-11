"""Run the retained law against a private mutation of the actual transition."""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

SOURCES = (
    "crates/arbos-relay/src/restart_lifetime.rs",
    ".improve/state-machine-checks/restart_state_bridge.rs",
    "harness/arbos-pi-ext/restart-state-laws.test.ts",
)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    root, out = args.root.resolve(), args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    before = {name: digest(root / name) for name in SOURCES}
    results = []
    with tempfile.TemporaryDirectory(prefix="arbos-restart-mutant-") as directory:
        fixture = Path(directory)
        for name in SOURCES:
            target = fixture / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(root / name, target)
        harness = fixture / "harness/arbos-pi-ext"
        (harness / "node_modules").symlink_to(root / "harness/arbos-pi-ext/node_modules", target_is_directory=True)
        (harness / "package.json").write_text('{"private":true,"type":"module"}\n')
        command = ["node", "--test", "--test-name-pattern=stale planned cleanup", "restart-state-laws.test.ts"]
        target = fixture / SOURCES[0]
        original = target.read_text()
        anchor = "(State::Planned { generation, .. }, Event::Release(owner)) if generation == owner => {"
        if original.count(anchor) != 1:
            raise RuntimeError("Actual reducer mutation anchor changed")
        for variant in ("control", "mutant"):
            if variant == "mutant":
                target.write_text(original.replace(anchor,
                    "(State::Planned { generation, .. }, Event::Release(_owner)) => {", 1))
            start = time.monotonic()
            guarded = (["/usr/bin/sandbox-exec", "-p", "(version 1) (allow default) (deny network*)"] if sys.platform == "darwin" else []) + command
            run = subprocess.run(guarded, cwd=harness, env={"PATH": os.environ.get("PATH", "/usr/bin:/bin")},
                                 text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=30)
            elapsed = time.monotonic() - start
            (out / (variant + ".log")).write_text(run.stdout)
            metadata = {"variant": variant, "command": command, "exit": run.returncode,
                        "seconds": elapsed, "actual_transition_source_sha256": digest(target)}
            if variant == "control":
                if run.returncode != 0 or "# pass 1" not in run.stdout:
                    raise RuntimeError("Unmodified control did not pass its 256 generated cases")
                metadata["generated_cases"] = 256
            else:
                prefix = "# restart-counterexample:"
                records = [json.loads(base64.b64decode(line[len(prefix):], validate=True))
                           for line in run.stdout.splitlines() if line.startswith(prefix)]
                if len(records) != 1:
                    raise RuntimeError("Missing generated mutation counterexample")
                record = records[0]
                failed, replay = record["result"], record["replay"]
                if (run.returncode == 0 or failed["_tag"] != "Falsified" or replay["_tag"] != "Falsified"
                        or failed["failure"]["_tag"] != "ReturnedFalse"
                        or replay["failure"]["_tag"] != "ReturnedFalse"
                        or failed["shrunkInput"] != replay["shrunkInput"]):
                    raise RuntimeError("Mutation was not falsified and replayed as a semantic counterexample")
                metadata["counterexample"] = record
            results.append(metadata)
    after = {name: digest(root / name) for name in SOURCES}
    if before != after:
        raise RuntimeError("Input source changed during the mutation check")
    (out / "mutant.json").write_text(json.dumps({"source_unchanged": True, "source_sha256": after,
                                               "results": results}, indent=2) + "\n")
    print(json.dumps({"control_cases": 256, "actual_function_mutants": 1, "falsified_and_replayed": True}))


if __name__ == "__main__":
    main()

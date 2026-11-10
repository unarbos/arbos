"""Run the actual std-only Mac policies, with optional independent negative controls."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
OUT = Path(__file__).resolve().parent / "generated"
MODULES = {"chat_title": ("chat_title.rs", 8), "mentions": ("mentions.rs", 3),
           "agent_link_expiry": ("agent_link_expiry.rs", 2),
           "pending_messages": ("pending_messages.rs", 5),
           "snapshot_schedule": ("snapshot_schedule.rs", 7),
           "room_docs": ("sync/room_docs.rs", 7)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mutants", action="store_true")
    args = parser.parse_args()
    OUT.mkdir(exist_ok=True)
    results = []
    with tempfile.TemporaryDirectory(prefix="arbos-mac-policies-") as directory:
        temporary = Path(directory)
        for name, (relative_path, expected) in MODULES.items():
            source = ROOT / "crates/arbos-mac/src" / relative_path
            binary = temporary / name
            started = time.monotonic()
            command = ["rustc", "--edition=2021", "--test", "-D", "warnings", str(source), "-o", str(binary)]
            compiled = subprocess.run(command, capture_output=True, text=True, timeout=60)
            if compiled.returncode:
                raise RuntimeError(compiled.stdout + compiled.stderr)
            compile_seconds = time.monotonic() - started
            started = time.monotonic()
            run = subprocess.run([str(binary)], capture_output=True, text=True, timeout=10)
            (OUT / f"{name}.log").write_text(run.stdout + run.stderr)
            counts = re.search(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored", run.stdout)
            if run.returncode or counts is None or tuple(map(int, counts.groups())) != (expected, 0, 0):
                raise RuntimeError(run.stdout + run.stderr)
            results.append({"module": name, "source_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
                            "passed": expected, "failed": 0, "ignored": 0,
                            "compile_seconds": compile_seconds, "test_seconds": time.monotonic() - started})
        mutants = []
        if args.mutants:
            source = (ROOT / "crates/arbos-mac/src/snapshot_schedule.rs").read_text()
            changes = [("always-paint", "if caught_up {", "if true {"),
                       ("postpone-burst", "self.deadline.get_or_insert_with(|| now + WINDOW);", "self.deadline = Some(now + WINDOW);"),
                       ("never-timer", "self.deadline.take().is_some() && !caught_up", "{ self.deadline = None; let _ = caught_up; false }")]
            for name, old, new in changes:
                if source.count(old) != 1:
                    raise RuntimeError(f"Mutation anchor changed: {name}")
                file = temporary / f"{name}.rs"
                binary = temporary / name
                file.write_text(source.replace(old, new, 1))
                compiled = subprocess.run(["rustc", "--edition=2021", "--test", str(file), "-o", str(binary)], capture_output=True, text=True, timeout=60)
                if compiled.returncode:
                    raise RuntimeError(compiled.stdout + compiled.stderr)
                run = subprocess.run([str(binary)], capture_output=True, text=True, timeout=10)
                (OUT / f"{name}.log").write_text(run.stdout + run.stderr)
                if run.returncode != 101 or "test result: FAILED." not in run.stdout:
                    raise RuntimeError(f"Tests did not detect actual-function mutant: {name}")
                mutants.append({"name": name, "detected": True})
    summary = {"modules": results, "total_passed": sum(row["passed"] for row in results), "mutants": mutants}
    (OUT / "GATES.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(summary))


if __name__ == "__main__":
    main()

"""Negative controls against isolated copies of the actual pure voice modules.

No live source is edited. Run after the harness frozen install and with Node/Python
available. Every fixture reuses the exact retained generated property and bridge.
"""
from pathlib import Path
import hashlib
import json
import os
import shutil
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
OUT = Path(__file__).resolve().parent / "generated"
OUT.mkdir(exist_ok=True)
SOURCES = (
    "harness/arbos-pi-ext/voice-state-laws.test.ts",
    "voice/tests/state_bridge.py",
    "voice/arbos_voice/__init__.py",
    "voice/arbos_voice/call_state.py",
    "voice/arbos_voice/vision_state.py",
)
MUTANTS = (
    ("vision-stale-generation", "voice/arbos_voice/vision_state.py",
     "return isinstance(state, Enabled) and generation == state.generation",
     "return isinstance(state, Enabled)", "revoked frame leases", 84002),
    ("failed-mute-commits", "voice/arbos_voice/call_state.py",
     "return ApplyFailed(state.previous, state.target, state.attempt)",
     "return Applied(state.target, state.attempt)", "failed mute is not committed", 84004),
    ("stale-audio-completion", "voice/arbos_voice/call_state.py",
     "if not isinstance(state, Applying) or state.attempt != event.attempt:",
     "if not isinstance(state, Applying):", "old (mute|mode) completions", 84005),
    ("closed-call-restarts", "voice/arbos_voice/call_state.py",
     "if isinstance(state, Closed):\n        return state",
     "if isinstance(state, Closed):\n        return Connecting()", "closed call and vision", 84001),
)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    before = {source: digest(ROOT / source) for source in SOURCES}
    results = []
    for name, source, old, new, pattern, seed in MUTANTS:
        with tempfile.TemporaryDirectory(prefix="arbos-voice-mutant-") as directory:
            fixture = Path(directory)
            for path in SOURCES:
                target = fixture / path
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(ROOT / path, target)
            (fixture / "harness/arbos-pi-ext/node_modules").symlink_to(ROOT / "harness/arbos-pi-ext/node_modules", target_is_directory=True)
            target = fixture / source
            original = target.read_text()
            if original.count(old) != 1:
                raise RuntimeError(f"Mutation anchor changed: {name}")
            target.write_text(original.replace(old, new, 1))
            command = ["node", "--test", f"--test-name-pattern={pattern}", "voice-state-laws.test.ts"]
            start = time.monotonic()
            run = subprocess.run(command, cwd=fixture / "harness/arbos-pi-ext", env={"PATH": os.environ.get("PATH", "/usr/bin:/bin")}, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=30)
            elapsed = time.monotonic() - start
            (OUT / f"mutant-{name}.log").write_text(run.stdout.replace(str(fixture), "$FIXTURE"))
            # A child transport failure must not masquerade as a falsified property.
            detected = run.returncode != 0 and "Falsified" in run.stdout and "Voice state observation failed" not in run.stdout
            results.append({"name": name, "actual_function_source": source, "isolated_mutant_sha256": digest(target), "seed": seed, "command": command, "exit_code": run.returncode, "wall_seconds": elapsed, "property_falsified": detected})
            if not detected:
                raise RuntimeError(f"Mutation did not produce a property counterexample: {name}")
    after = {source: digest(ROOT / source) for source in SOURCES}
    if before != after:
        raise RuntimeError("Live source changed while running isolated mutants")
    (OUT / "mutants.json").write_text(json.dumps({"live_source_unchanged": True, "live_source_sha256": after, "mutants": results}, indent=2) + "\n")
    print(json.dumps({"negative_controls": len(results), "all_falsified": True, "live_source_unchanged": True}))


if __name__ == "__main__":
    main()

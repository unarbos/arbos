#!/usr/bin/env python3
"""Offline leaf-module gates and paired microbenchmarks; no services or user data."""

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import random
import re
import shutil
import statistics
import subprocess
import time

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
SOURCE = ROOT / "crates/arbos-core/src/redact.rs"
RESULTS = HERE / "results"
TARGET = HERE / "target"
MANIFEST = HERE / "Cargo.toml"
ENV = dict(os.environ, CARGO_TARGET_DIR=str(TARGET), CARGO_BUILD_JOBS="2", CARGO_INCREMENTAL="0")


def checked(command, *, log=None):
    started = time.perf_counter()
    result = subprocess.run(command, cwd=ROOT, env=ENV, text=True, capture_output=True)
    output = result.stdout + result.stderr
    if log:
        log.write_text(output)
    if result.returncode:
        raise RuntimeError(f"{command} failed ({result.returncode}):\n{output}")
    return output, round(time.perf_counter() - started, 6)


def metadata():
    def get(*command):
        return checked(list(command))[0].strip()
    return {"source_sha256": hashlib.sha256(SOURCE.read_bytes()).hexdigest(),
            "git_commit": get("git", "rev-parse", "HEAD"), "rustc": get("rustc", "--version"),
            "platform": platform.platform(), "machine": platform.machine(),
            "cpu": get("sysctl", "-n", "machdep.cpu.brand_string") if platform.system() == "Darwin" else platform.processor(),
            "cpu_count": os.cpu_count()}


def capture(args):
    result_dir = RESULTS / args.label
    result_dir.mkdir(parents=True, exist_ok=True)
    records = []
    commands = [
        ("fmt", ["fmt", "--all", "--check"]),
        ("clippy", ["clippy", "--all-targets", "--", "-D", "warnings"]),
        ("check", ["check", "--all-targets"]),
        ("test", ["test"]),
        ("build", ["build"]),
        ("release-benchmark-build", ["build", "--release", "--bin", "redact-bench"]),
    ]
    for name, tail in commands:
        command = ["cargo", tail[0], "--manifest-path", str(MANIFEST)]
        if name != "fmt":
            command += ["--locked", "--offline"]
        command += tail[1:]
        output, seconds = checked(["nice", "-n", "12", *command], log=result_dir / f"{name}.log")
        record = {"gate": name, "command": command, "seconds": seconds, "exit_code": 0}
        if name == "test":
            counts = re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored", output)
            record["counts"] = {key: sum(int(c[i]) for c in counts)
                                for i, key in enumerate(["pass", "fail", "ignored"])}
        records.append(record)
    binary = TARGET / f"{args.label}-redact-bench"
    shutil.copy2(TARGET / "release/redact-bench", binary)
    output, seconds = checked(["nice", "-n", "12", str(binary), str(args.iterations), str(args.rounds)])
    result = {"metadata": metadata(), "gates": records, "benchmark_seconds": seconds,
              "benchmark": json.loads(output), "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest()}
    (result_dir / "capture.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"label": args.label, "gates": records, "benchmark_seconds": seconds,
                      "fingerprint": result["benchmark"]["fingerprint"]}, indent=2))


def percentile(values, p):
    values = sorted(values)
    at = (len(values) - 1) * p
    lo = math.floor(at)
    return values[lo] + (values[math.ceil(at)] - values[lo]) * (at - lo)


def compare(args):
    rng = random.Random(20261109)
    pairs = []
    fingerprints = set()
    for pair in range(args.rounds):
        labels = ["before", "after"]
        rng.shuffle(labels)
        observed = {}
        for label in labels:
            output, _ = checked(["nice", "-n", "12", str(TARGET / f"{label}-redact-bench"), str(args.iterations), "1"])
            measured = json.loads(output)
            fingerprints.add(measured["fingerprint"])
            observed[label] = measured
        pairs.append({"pair": pair, "order": labels, "observations": observed})
    if len(fingerprints) != 1:
        raise RuntimeError(f"Behavior fingerprints differ: {fingerprints}")
    summary = {}
    for workload in ["redact_patterns", "find_key_like", "redact_placeholders", "scrub_value"]:
        values = {}
        for label in ["before", "after"]:
            values[label] = [next(s["total_ns"] / s["iterations"] for s in p["observations"][label]["samples"]
                                  if s["workload"] == workload) for p in pairs]
        deltas = [a - b for a, b in zip(values["before"], values["after"])]
        bootstrap = [statistics.median(rng.choices(deltas, k=len(deltas))) for _ in range(2000)]
        nonzero = [d for d in deltas if d != 0]
        smaller = min(sum(d > 0 for d in nonzero), sum(d < 0 for d in nonzero))
        p_value = min(1.0, 2 * sum(math.comb(len(nonzero), k) for k in range(smaller + 1)) / 2 ** len(nonzero))
        summary[workload] = {"units": "nanoseconds per call",
            **{label: {"median": statistics.median(v), "min": min(v), "max": max(v)} for label, v in values.items()},
            "median_speedup_ratio": statistics.median(values["before"]) / statistics.median(values["after"]),
            "paired_median_saved_ns": statistics.median(deltas),
            "paired_median_saved_ns_bootstrap_95_ci": [percentile(bootstrap, .025), percentile(bootstrap, .975)],
            "paired_two_sided_sign_test_p": p_value, "pairs": len(pairs)}
    result = {"seed": 20261109, "iterations_per_sample": args.iterations, "fingerprint": fingerprints.pop(),
              "metadata": metadata(), "summary": summary, "pairs": pairs,
              "limits": "Warm release microbenchmark of one actual source module, not app latency. scrub_value includes cloning a JSON fixture with two text leaves and scalar values; all inputs are deterministic synthetic cases."}
    (RESULTS / "comparison.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["capture", "compare"])
    parser.add_argument("label", nargs="?", choices=["before", "after"])
    parser.add_argument("--iterations", type=int, default=2000)
    parser.add_argument("--rounds", type=int, default=15)
    args = parser.parse_args()
    if args.iterations <= 0 or args.rounds <= 0:
        parser.error("iterations and rounds must be positive")
    if args.action == "capture" and not args.label:
        parser.error("capture requires before or after label")
    (capture if args.action == "capture" else compare)(args)

"""Paired warm comparison of fixed uncached versus fixed cached policy; no services."""

import hashlib
import json
import math
import os
from pathlib import Path
import platform
import random
import statistics
import subprocess
import time

HERE = Path(__file__).resolve().parent
SEED = 20261109
ITERATIONS = 2000
PAIRS = 21


def percentile(values, p):
    values = sorted(values)
    position = (len(values) - 1) * p
    lower = math.floor(position)
    return values[lower] + (values[math.ceil(position)] - values[lower]) * (position - lower)


rng = random.Random(SEED)
pairs = []
fingerprints = set()
started = time.perf_counter()
for number in range(PAIRS):
    if time.perf_counter() - started > 29:
        raise RuntimeError("Bounded timing window exceeded; no complete comparison claimed")
    order = ["uncached", "cached"]
    rng.shuffle(order)
    observations = {}
    for label in order:
        command = ["nice", "-n", "12", str(HERE / f"{label}-redact-bench"), str(ITERATIONS), "1"]
        child = subprocess.run(command, text=True, capture_output=True, check=True, timeout=5)
        observed = json.loads(child.stdout)
        assert observed["corpus_cases"] == 130
        fingerprints.add(observed["fingerprint"])
        observations[label] = observed
    pairs.append({"pair": number, "order": order, "observations": observations})
    (HERE / "raw-pairs.json").write_text(json.dumps(pairs, indent=2) + "\n")
timing_seconds = time.perf_counter() - started
assert len(fingerprints) == 1, "actual output fingerprints must agree"
assert (HERE / "uncached-outputs.json").read_bytes() == (HERE / "cached-outputs.json").read_bytes()

summary = {}
for workload in ("redact_patterns", "find_key_like", "redact_placeholders", "scrub_value"):
    values = {
        label: [next(s["total_ns"] / s["iterations"] for s in p["observations"][label]["samples"]
                     if s["workload"] == workload) for p in pairs]
        for label in ("uncached", "cached")
    }
    deltas = [a - b for a, b in zip(values["uncached"], values["cached"])]
    bootstrap = [statistics.median(rng.choices(deltas, k=len(deltas))) for _ in range(2000)]
    nonzero = [d for d in deltas if d != 0]
    smaller = min(sum(d > 0 for d in nonzero), sum(d < 0 for d in nonzero))
    p_value = min(1.0, 2 * sum(math.comb(len(nonzero), k) for k in range(smaller + 1)) / 2 ** len(nonzero))
    summary[workload] = {
        "units": "nanoseconds per call",
        **{label: {"median": statistics.median(v), "min": min(v), "max": max(v)}
           for label, v in values.items()},
        "median_speedup_ratio": statistics.median(values["uncached"]) / statistics.median(values["cached"]),
        "paired_median_saved_ns": statistics.median(deltas),
        "paired_median_saved_ns_bootstrap_95_ci": [percentile(bootstrap, .025), percentile(bootstrap, .975)],
        "paired_two_sided_sign_test_p": p_value,
        "pairs": len(pairs),
    }

result = {
    "seed": SEED, "pairs": PAIRS, "iterations_per_sample": ITERATIONS,
    "timing_seconds": timing_seconds, "warm_release_only": True, "corpus_cases": 130,
    "original_phase6_cases": 109, "additional_minimum_tail_cases": 21,
    "fingerprint": fingerprints.pop(),
    "exact_outputs_sha256": hashlib.sha256((HERE / "cached-outputs.json").read_bytes()).hexdigest(),
    "same_exact_output_bytes": True,
    "machine": {"platform": platform.platform(), "machine": platform.machine(), "cpu_count": os.cpu_count(),
                "cpu": (subprocess.check_output(["sysctl", "-n", "machdep.cpu.brand_string"], text=True).strip()
                        if platform.system() == "Darwin" else platform.processor() or platform.machine()),
                "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip()},
    "summary": summary, "raw_pairs": "raw-pairs.json",
    "limits": "Fixed-policy cache comparison, not old versus corrected policy. One machine and one exclusive team timing window, no cross-device replication. JSON workload includes cloning the synthetic two-string-leaf fixture. Warm module microbenchmarks are not whole-app latency. All raw values/order retained; bootstrap intervals describe paired local samples, not production confidence bounds.",
}
(HERE / "RESULTS.json").write_text(json.dumps(result, indent=2) + "\n")
print(json.dumps(result, indent=2))

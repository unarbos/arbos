"""Check/measure the real URL wrapper without compiling GPUI (Python stdlib + rustc).

python3 .improve/perf/urlwrap/measure.py
python3 .improve/perf/urlwrap/measure.py --baseline-ref <commit> --benchmark --output results.json
"""

from pathlib import Path
import argparse
import hashlib
import json
import statistics
import subprocess
import tempfile
import time


ROOT = Path(__file__).resolve().parents[3]
SOURCE = Path("crates/arbos-mac/src/ui/urlwrap.rs")
DRIVER = Path(__file__).with_name("driver.rs")


def run(command, **kwargs):
    result = subprocess.run(command, capture_output=True, text=True, **kwargs)
    if result.returncode:
        raise SystemExit(f"{command[0]} exited {result.returncode}:\n{result.stdout}{result.stderr}")
    return result.stdout


def summarize(samples):
    groups = {}
    for row in samples:
        groups.setdefault((row["scheme"], row["links"], row["input_bytes"]), []).append(row["ns_per_call"])
    return [dict(scheme=key[0], links=key[1], input_bytes=key[2], samples=len(values),
                 median_ns=round(statistics.median(values), 3), min_ns=min(values),
                 p95_ns=sorted(values)[int((len(values) - 1) * 0.95)])
            for key, values in groups.items()]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline-ref", help="compare the unmodified source at this local git commit")
    parser.add_argument("--benchmark", action="store_true", help="measure 31 samples at 1/100/1000 URLs")
    parser.add_argument("--output", type=Path, help="save JSON evidence; otherwise print it")
    args = parser.parse_args()
    sources = {"current": (ROOT / SOURCE).read_text()}
    if args.baseline_ref:
        sources = {"before": run(["git", "show", f"{args.baseline_ref}:{SOURCE.as_posix()}"], cwd=ROOT),
                   "after": sources["current"]}
    result = {"rustc": run(["rustc", "--version"]).strip(), "source": SOURCE.as_posix(),
              "baseline_ref": args.baseline_ref, "results": {},
              "limits": "Isolated optimized URL-formatting microbenchmark; not app open/send latency or GPUI visual verification."}
    with tempfile.TemporaryDirectory(prefix="arbos-urlwrap-") as temp:
        temp = Path(temp)
        binaries = {}
        for label, source in sources.items():
            production = temp / f"{label}-urlwrap.rs"
            production.write_text(source)
            wrapper = temp / f"{label}-driver.rs"
            wrapper.write_text(f"#[path = {json.dumps(str(production), ensure_ascii=False)}] mod urlwrap;\n"
                               f"include!({json.dumps(str(DRIVER), ensure_ascii=False)});\n")
            binary = temp / label
            started = time.perf_counter()
            run(["rustc", "--edition=2021", "-C", "opt-level=3", "-D", "warnings", str(wrapper), "-o", str(binary)])
            result["results"][label] = {"source_sha256": hashlib.sha256(source.encode()).hexdigest(),
                                        "compile_seconds": round(time.perf_counter() - started, 3),
                                        "checks": json.loads(run([str(binary), "check"]))}
            binaries[label] = binary
        if args.benchmark:
            result["method"] = "31 paired samples; before/after order alternates each round; 8 warmups per workload; 5000/links calls per sample."
            for label in binaries:
                result["results"][label]["samples"] = []
            for sample in range(31):
                labels = list(binaries)
                if sample % 2:
                    labels.reverse()
                for label in labels:
                    rows = [json.loads(line) for line in run([str(binaries[label]), "bench", str(sample)]).splitlines()]
                    result["results"][label]["samples"].extend(rows)
            for label in binaries:
                result["results"][label]["summary"] = summarize(result["results"][label]["samples"])
    output = json.dumps(result, indent=2) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(output)
        print(json.dumps({"output": str(args.output), "checks": {k: v["checks"] for k, v in result["results"].items()}}))
    else:
        print(output, end="")


if __name__ == "__main__":
    main()

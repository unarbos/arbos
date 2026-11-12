#!/usr/bin/env python3
"""Compare actual pure agent parsers before/after a fixed-regex cache change."""
import argparse
from collections import Counter
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import random
import re
import shutil
import socket
import statistics
import subprocess
import time

HERE = Path(__file__).resolve().parent
BASE = "2865cb29b806121d4a90f55ba6ce151a150279a0"
FILES = {name: f"crates/arbos-agentd/src/{name}.rs" for name in ("web", "tree", "llm")}
FUNCTIONS = {"web": ("html_to_text", "strip_tags", "absolute_url", "decode_entities"),
             "tree": ("parse_card",), "llm": ("parse_json_loose",)}


def function(source, name):
    match = re.search(r"^(?:pub )?(?:async )?fn " + name + r"\(", source, re.M)
    if match is None:
        raise ValueError(f"Missing production function {name}")
    return source[match.start():source.index("\n}", match.start()) + 2]


def digest(value):
    return hashlib.sha256(value).hexdigest()


def patterns(source):
    values = []
    for constructor in re.finditer(r"regex_lite::Regex::new\(", source):
        match = re.match(r'\s*(?:&format!\(\s*)?(r(#{0,5})"[\s\S]*?"\2)', source[constructor.end():])
        if match is None:
            raise ValueError("Expected a fixed raw pattern or fixed tag-format pattern")
        values.append(match.group(1))
    return Counter(values)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=HERE.parents[2])
    parser.add_argument("--out", type=Path, required=True, help="fresh private directory")
    parser.add_argument("--before-ref", default=BASE)
    parser.add_argument("--measure", action="store_true")
    args = parser.parse_args()
    root, out = args.root.resolve(), args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    if platform.system() == "Darwin":
        fence = ["/usr/bin/sandbox-exec", "-p", "(version 1) (allow default) (deny network*)"]
    elif {name for _, name in socket.if_nameindex()} == {"lo"}:
        fence = []
    else:
        raise SystemExit("Use macOS or the project's loopback-only Linux test namespace")
    for name in ("home", "tmp"):
        (out / name).mkdir()
    env = {"PATH": os.environ["PATH"], "HOME": str(out / "home"), "TMPDIR": str(out / "tmp"),
           "CARGO_HOME": os.environ.get("CARGO_HOME", str(Path.home() / ".cargo")),
           "RUSTUP_HOME": os.environ.get("RUSTUP_HOME", str(Path.home() / ".rustup")),
           "CARGO_BUILD_JOBS": "2", "CARGO_INCREMENTAL": "0"}
    def command(arguments, log):
        start = time.monotonic()
        result = subprocess.run(fence + list(map(str, arguments)), env=env, capture_output=True, text=True)
        (out / log).write_text(result.stdout + result.stderr)
        if result.returncode:
            raise SystemExit(f"Command failed; see {out / log}")
        return result.stdout, time.monotonic() - start
    sources = {
        "before": {name: subprocess.run(["git", "show", f"{args.before_ref}:{path}"], cwd=root,
                                       check=True, capture_output=True).stdout.decode()
                   for name, path in FILES.items()},
        "after": {name: (root / path).read_text() for name, path in FILES.items()},
    }
    for name in ("web", "tree"):
        assert patterns(sources["before"][name]) == patterns(sources["after"][name]), name
    results = {"scope": "selected actual pure functions and search regex bindings; no HTTP, agent, database or application execution",
               "fixed_pattern_inventory_equal": True, "source_sha256": {}, "controls": {}, "build_seconds": {}}
    for arm, files in sources.items():
        work = out / arm
        (work / "src").mkdir(parents=True)
        results["source_sha256"][arm] = {FILES[n]: digest(v.encode()) for n, v in files.items()}
        for name, names in FUNCTIONS.items():
            source = files[name]
            prefix = "use serde_json::Value;\n" if name != "web" else ""
            if name == "web" and "static HTML_OMITTED:" in source:
                prefix += "use std::sync::LazyLock;\n" + source[source.index("static HTML_OMITTED:"):source.index("const DEFAULT_SEARCH_MCP_URL:")]
            if name == "tree" and "static CARD_HEADING:" in source:
                prefix += "use std::sync::LazyLock;\n" + source[source.index("static CARD_HEADING:"):source.index("pub const SCHEMA:")]
            (work / "src" / f"{name}.rs").write_text(prefix + "\n\n".join(function(source, n) for n in names) + "\n")
        web = files["web"]
        start = web.index("    let link_re")
        bindings = web[start:web.index("    let snippets:", start)]
        template = (HERE / "search.rs").read_text()
        assert template.count("/* SOURCE_SEARCH_BINDINGS */") == 1
        (work / "src/search.rs").write_text(template.replace("/* SOURCE_SEARCH_BINDINGS */", bindings))
        shutil.copyfile(HERE / "fixture.rs", work / "src/main.rs")
        shutil.copyfile(HERE / "Cargo.lock", work / "Cargo.lock")
        (work / "Cargo.toml").write_text('[package]\nname="static-regex-check"\nversion="0.0.0"\nedition="2021"\n[workspace]\n[dependencies]\nregex-lite="0.1"\nserde_json="1"\n')
        env["CARGO_TARGET_DIR"] = str(out / f"{arm}-target")
        _, seconds = command(["cargo", "build", "--locked", "--offline", "--release", "--manifest-path", work / "Cargo.toml"], f"{arm}-build.log")
        results["build_seconds"][arm] = seconds
        value, _ = command([out / f"{arm}-target/release/static-regex-check"], f"{arm}-controls.log")
        results["controls"][arm] = json.loads(value)
    assert results["controls"]["before"] == results["controls"]["after"]
    expected = results["controls"]["before"]
    results["output_sha256"] = digest(json.dumps(expected, sort_keys=True, separators=(",", ":")).encode())
    if args.measure:
        rng, samples = random.Random(61110), []
        for i in range(21):
            arms = ["before", "after"]
            rng.shuffle(arms)
            row = {"pair": i, "order": arms}
            for arm in arms:
                value, _ = command([out / f"{arm}-target/release/static-regex-check", "bench"], f"sample-{i}-{arm}.log")
                value = json.loads(value)
                assert value["controls"] == expected
                row[arm] = value["timings"]
            samples.append(row)
        results["samples"], results["measurements"] = samples, {}
        for name in samples[0]["before"]:
            bs = [r["before"][name]["nanoseconds_per_call"] for r in samples]
            ats = [r["after"][name]["nanoseconds_per_call"] for r in samples]
            differences = [b-a for b, a in zip(bs, ats)]
            boot = sorted(statistics.median(rng.choices(differences, k=21)) for _ in range(20000))
            positives = sum(d > 0 for d in differences)
            p = min(1, 2*sum(math.comb(21, k) for k in range(min(positives, 21-positives)+1))/2**21)
            results["measurements"][name] = {"before_median_ns": statistics.median(bs), "after_median_ns": statistics.median(ats),
                                               "paired_median_saving_95ci_ns": [boot[500], boot[19499]], "two_sided_sign_test_p": p}
    assert all((root / FILES[n]).read_text() == s for n, s in sources["after"].items())
    (out / "RESULTS.json").write_text(json.dumps(results, indent=2) + "\n")
    print(json.dumps({"controls": "PASS", "measured": args.measure, "result": str(out / "RESULTS.json")}))


if __name__ == "__main__":
    main()

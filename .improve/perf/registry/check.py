#!/usr/bin/env python3
"""Replay selected production registry code with controlled async JSON responses."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import random
import shutil
import socket
import statistics
import subprocess
import time

HERE = Path(__file__).resolve().parent
BASE = "2865cb29b806121d4a90f55ba6ce151a150279a0"
FUNCTIONS = ("refresh_registry", "fetch_unread_map", "align_people_archive",
             "project_image_url", "project_doc_key")
EXTRAS = ("hydrate_project", "fetch_people_list")
RELAY = "crates/arbos-mac/src/sync/relay.rs"


def function(source, name):
    import re
    match = re.search(r"^(?:async )?fn " + name + r"\(", source, re.M)
    if match is None:
        raise ValueError(f"missing production function: {name}")
    end = source.index("\n}", match.start()) + 2
    return source[match.start():end]


def digest(data):
    return hashlib.sha256(data).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=HERE.parents[2])
    parser.add_argument("--out", type=Path, required=True, help="fresh private output directory")
    parser.add_argument("--before-ref", default=BASE)
    parser.add_argument("--measure", action="store_true", help="21 randomized paired delay experiments")
    args = parser.parse_args()
    root, out = args.root.resolve(), args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    if platform.system() == "Darwin":
        fence = ["/usr/bin/sandbox-exec", "-p", "(version 1) (allow default) (deny network*)"]
    elif {name for _, name in socket.if_nameindex()} == {"lo"}:
        fence = []  # The caller already placed this process in a loopback-only namespace.
    else:
        raise SystemExit("Run on macOS or inside the project's loopback-only Linux test namespace.")
    for folder in ("home", "tmp"):
        (out / folder).mkdir()
    env = {"PATH": os.environ["PATH"], "HOME": str(out / "home"), "TMPDIR": str(out / "tmp"),
           "CARGO_HOME": os.environ.get("CARGO_HOME", str(Path.home() / ".cargo")),
           "RUSTUP_HOME": os.environ.get("RUSTUP_HOME", str(Path.home() / ".rustup")),
           "CARGO_BUILD_JOBS": "2", "CARGO_PROFILE_DEV_DEBUG": "0", "CARGO_INCREMENTAL": "0"}
    def command(argv, log):
        start = time.monotonic()
        result = subprocess.run(fence + list(map(str, argv)), env=env, text=True, capture_output=True)
        (out / log).write_text(result.stdout + result.stderr)
        if result.returncode:
            raise SystemExit(f"Failed {argv[0]}: see {out / log}")
        return result.stdout, time.monotonic() - start
    before = subprocess.run(["git", "show", f"{args.before_ref}:{RELAY}"], cwd=root,
                            check=True, capture_output=True).stdout.decode()
    after_bytes = (root / RELAY).read_bytes()
    sources = {"before": before, "after": after_bytes.decode()}
    hashes, controls, builds = {}, {}, {}
    for arm, source in sources.items():
        work = out / arm
        (work / "src/sync").mkdir(parents=True)
        for name in ("main.rs", "fixture.rs"):
            shutil.copyfile(HERE / name, work / "src" / ("relay.rs" if name == "fixture.rs" else name))
        selected = list(FUNCTIONS) + ([] if arm == "before" else list(EXTRAS))
        bodies = {name: function(source, name) for name in selected}
        (work / "src/actual.rs").write_text("\n\n".join(bodies.values()) + "\n")
        hashes[arm] = {name: digest(body.encode()) for name, body in bodies.items()}
        for name, relative in (("model.rs", "crates/arbos-mac/src/model.rs"),
                               ("dto.rs", "crates/arbos-mac/src/sync/dto.rs"),
                               ("agent_avatar.rs", "crates/arbos-mac/src/sync/agent_avatar.rs")):
            shutil.copyfile(root / relative, work / "src" / name)
        credentials = (root / "crates/arbos-mac/src/sync/credentials.rs").read_text()
        start = credentials.index("#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]")
        stop = credentials.index("\npub fn config_dir()")
        (work / "src/credentials.rs").write_text("use serde::{Serialize, Deserialize};\nuse crate::model::now_ms;\n" + credentials[start:stop])
        diagnostics = (root / "crates/arbos-mac/src/sync/diagnostics.rs").read_text()
        (work / "src/diagnostics.rs").write_text("pub " + function(diagnostics.replace("pub fn is_transient", "fn is_transient"), "is_transient") + "\n")
        for name in ("project_json.json", "project_members_json.json", "people_chat_json_with_me.json", "chat_json.json"):
            shutil.copyfile(root / "crates/arbos-relay/tests/fixtures/api" / name, work / "src" / name)
        manifest = '''[package]
name="perf-async-check"
version="0.0.0"
edition="2021"
[workspace]
[dependencies]
anyhow="1"
serde={version="1",features=["derive"]}
serde_json="1"
chrono="0.4"
ulid="1"
log="0.4"
futures="0.3"
tokio={version="1",features=["rt","macros","sync","time"]}
arbos-core={path=CORE}
'''.replace("CORE", json.dumps(str(root / "crates/arbos-core")))
        (work / "Cargo.toml").write_text(manifest)
        shutil.copyfile(HERE / "Cargo.lock", work / "Cargo.lock")
        env["CARGO_TARGET_DIR"] = str(out / f"{arm}-target")
        _, builds[arm] = command(["cargo", "build", "--locked", "--offline", "--manifest-path", work / "Cargo.toml"], f"{arm}-build.log")
        binary = out / f"{arm}-target/debug/perf-async-check"
        controls[arm] = {}
        for mode in ("controls", "cancel", "bounds"):
            arguments = [] if mode == "controls" else (["bench", "8"] if mode == "bounds" else [mode])
            value, _ = command([binary] + arguments, f"{arm}-{mode}.log")
            controls[arm][mode] = json.loads(value)
        assert controls[arm]["bounds"]["cases"][0]["peak"] == (1 if arm == "before" else 5)
    def normalized(value):
        return [{k: v for k, v in c.items() if k not in ("milliseconds", "peak")} for c in value["cases"]]
    assert normalized(controls["before"]["controls"]) == normalized(controls["after"]["controls"])
    assert normalized(controls["before"]["bounds"]) == normalized(controls["after"]["bounds"])
    result = {"function_sha256": hashes, "build_seconds": builds, "controls": controls,
              "exact_observable_output_equal": True, "network": "OS-enforced isolation",
              "scope": "selected actual functions and typed mappers; scripted gateway/storage/events, no GUI/auth/network execution"}
    if args.measure:
        rng, samples, expected = random.Random(61109), [], None
        for i in range(21):
            arms = ["before", "after"]
            rng.shuffle(arms)
            row = {"pair": i, "order": arms}
            for arm in arms:
                binary = out / f"{arm}-target/debug/perf-async-check"
                value, _ = command([binary, "bench", "8"], f"sample-{i}-{arm}.log")
                value = json.loads(value)
                if expected is None:
                    expected = normalized(value)
                assert normalized(value) == expected
                case = value["cases"][0]
                assert case["peak"] == (1 if arm == "before" else 5)
                row[arm] = {k: case[k] for k in ("milliseconds", "peak")}
            samples.append(row)
        bs = [r["before"]["milliseconds"] for r in samples]
        ats = [r["after"]["milliseconds"] for r in samples]
        differences = [b-a for b, a in zip(bs, ats)]
        boot = sorted(statistics.median(rng.choices(differences, k=21)) for _ in range(20000))
        positives = sum(d > 0 for d in differences)
        import math
        p = min(1, 2*sum(math.comb(21, k) for k in range(min(positives, 21-positives)+1))/2**21)
        result["measurement"] = {"pairs": samples, "before_median_ms": statistics.median(bs),
                                 "after_median_ms": statistics.median(ats),
                                 "paired_median_saving_95ci_ms": [boot[500], boot[19499]],
                                 "two_sided_sign_test_p": p,
                                 "scope": "12 projects + one people chat, 27 scripted 8ms requests; not app latency"}
    assert (root / RELAY).read_bytes() == after_bytes, "production source changed during execution"
    (out / "RESULTS.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"output": str(out / "RESULTS.json"), "controls": "PASS", "measured": args.measure}))


if __name__ == "__main__":
    main()

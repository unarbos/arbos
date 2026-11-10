#!/usr/bin/env python3
"""Exercise the actual JSONL process, including bounded input and error recovery."""

import argparse
import json
from pathlib import Path
import subprocess
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--binary",
        type=Path,
        default=Path(__file__).resolve().parent / "target/release/laws-bridge",
    )
    args = parser.parse_args()
    requests = []
    checks = []

    def encode(value):
        return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode()

    def add(name, request, check):
        requests.append(request if isinstance(request, bytes) else encode(request))
        checks.append((name, check))

    def invalid(request_id):
        return lambda response: response == {
            "id": request_id,
            "error": {"tag": "InvalidRequest", "message": "Invalid request"},
        }

    token = "ghp_" + "a" * 20
    add(
        "text-recognition-removal-and-utf8-range",
        {"id": 1, "op": "text", "text": f"🙂 token {token} end"},
        lambda r: r["id"] == 1
        and len(r["keys"]) == 1
        and r["keys"][0]["confident"]
        and r["keys"][0]["start"] == 11
        and r["keys"][0]["end"] == 11 + len(token)
        and r["keys"][0]["value"] == token
        and "ghp_" not in r["scrub"]
        and r["scrub"] == r["scrub_twice"],
    )
    add(
        "json-keys-and-nonstrings",
        {"id": 2, "op": "json", "value": {"op://key/name": [False, None, 17, "op://x/y"]}},
        lambda r: r == {
            "id": 2,
            "redact_json": {"op://key/name": [False, None, 17, "[vault reference]"]},
        },
    )
    add(
        "url-visible-text-and-hints",
        {"id": 3, "op": "url", "text": "é https://a.io/p?x=1&y=2 🙂"},
        lambda r: r["id"] == 3
        and r["plain"] == "é https://a.io/p?x=1&y=2 🙂"
        and "\u034f" in r["hinted"]
        and "\u200b" in r["hinted"],
    )
    invalid_cases = [
        ("not-json", b"{", None),
        ("empty-line", b"", None),
        ("invalid-utf8", b"\xff", None),
        ("lone-surrogate", b'{"id":4,"op":"text","text":"\\ud800"}', None),
        ("array-request", [], None),
        ("null-request", None, None),
        ("negative-id", {"id": -1, "op": "text", "text": ""}, None),
        ("fractional-id", {"id": 1.5, "op": "text", "text": ""}, None),
        ("unsafe-id", {"id": 9_007_199_254_740_992, "op": "text", "text": ""}, None),
        ("missing-op", {"id": 5, "text": ""}, 5),
        ("unknown-op", {"id": 6, "op": "unknown", "text": ""}, 6),
        ("unknown-field", {"id": 7, "op": "text", "text": "", "extra": False}, 7),
        ("missing-payload", {"id": 8, "op": "text"}, 8),
        ("wrong-payload-type", {"id": 9, "op": "text", "text": False}, 9),
    ]
    for name, request, request_id in invalid_cases:
        add(name, request, invalid(request_id))

    base = encode({"id": 10, "op": "url", "text": ""})
    add(
        "exact-byte-boundary",
        base + b" " * (65_536 - len(base)),
        lambda r: r == {"id": 10, "hinted": "", "plain": "", "breakable": ""},
    )
    add("oversized-line-drained", base + b" " * (65_537 - len(base)), invalid(None))
    add(
        "recovery-after-oversized-line",
        {"id": 11, "op": "json", "value": True},
        lambda r: r == {"id": 11, "redact_json": True},
    )
    for depth in (32, 33):
        value = 0
        for _ in range(depth):
            value = [value]
        check = (
            (lambda r, value=value, depth=depth: r == {"id": depth, "redact_json": value})
            if depth == 32
            else invalid(depth)
        )
        add(f"json-depth-{depth}", {"id": depth, "op": "json", "value": value}, check)
    add(
        "maximum-safe-id",
        {"id": 9_007_199_254_740_991, "op": "json", "value": None},
        lambda r: r == {"id": 9_007_199_254_740_991, "redact_json": None},
    )
    add(
        "eof-without-final-newline",
        {"id": 0, "op": "json", "value": []},
        lambda r: r == {"id": 0, "redact_json": []},
    )

    start = time.monotonic()
    child = subprocess.run(
        [str(args.binary.resolve())],
        input=b"\n".join(requests),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=10,
        check=False,
    )
    lines = child.stdout.splitlines()
    assert child.returncode == 0, "bridge must exit successfully at EOF"
    assert not child.stderr, "bridge must not log input or parser details"
    assert len(lines) == len(checks), "bridge must return exactly one response per line"
    results = []
    for line, (name, check) in zip(lines, checks):
        try:
            passed = bool(check(json.loads(line)))
        except (ValueError, KeyError, TypeError, IndexError):
            passed = False
        results.append({"case": name, "passed": passed})
    failed = [r["case"] for r in results if not r["passed"]]
    print(json.dumps({
        "passed": len(results) - len(failed),
        "failed": len(failed),
        "duration_seconds": time.monotonic() - start,
        "cases": results,
    }, indent=2))
    assert not failed, f"failed protocol cases: {', '.join(failed)}"


if __name__ == "__main__":
    main()

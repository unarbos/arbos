#!/usr/bin/env python3
"""What may leave for the partner's Prometheus, checked on the text of prometheus/subnet-rules.yml
(check.sh runs it). rules_test.yml proves the same on labelled input; this fails on the rule itself,
before anyone has to think of the input that would show it.

  - a counter never leaves as it is: a running total would tie a box to its past
  - the only labels a rule may keep are `kind` and `slot`
  - every per-box rule keeps `slot` and nothing else, and blunts its value by the step named here
"""
import pathlib, re, sys

KEPT = {"kind", "slot"}  # labels a subnet:* series may carry
INSIDE = {"box", "name", "le"}  # labels used on the way, always aggregated away again
# Rounding of what one dot carries. The relay's side of this is SLOTS_EVERY in metrics.rs (5 min).
STEP = {"subnet:box_tokens_per_minute": "1000",  # tokens per minute
        "subnet:box_cpu_cores": "0.1",  # cores
        "subnet:box_memory_bytes": "536870912",  # 0.5 GiB
        "subnet:box_state": None}  # a state code, nothing to round

text = (pathlib.Path(__file__).parent / "prometheus/subnet-rules.yml").read_text()
text = "\n".join(line for line in text.splitlines() if not line.lstrip().startswith("#"))
rules = {name: " ".join(body.split()) for name, body in re.findall(r"- record: (\S+)\s+expr: >?(.*?)(?=\n\s+- record:|\n\s+- name:|\Z)", text, re.S)}
problems = []
for name, expr in rules.items():
    for counter in re.finditer(r"\b\w+_(?:total|bucket)\b", expr):
        # count() of a counter's series is a number of series, not the counter
        if not re.search(r"(rate|increase|count)\($", expr[:counter.start()]):
            problems.append(f"{name}: {counter.group()} leaves as a counter")
    labels = {l.strip() for group in re.findall(r"\b(?:by|group_left) \(([^)]*)\)", expr) for l in group.split(",") if l.strip()}
    if labels - KEPT - INSIDE:
        problems.append(f"{name}: keeps {sorted(labels - KEPT - INSIDE)}")
    outer = set(re.match(r"\w+ by \(([^)]*)\)", expr).group(1).split(", ")) if re.match(r"\w+ by \(", expr) else set()
    if labels & KEPT and not (outer and outer <= KEPT):
        problems.append(f"{name}: `kind` or `slot` is used but the outermost aggregation is not by one of them")
    if "slot" in labels or name in STEP:
        if outer != {"slot"}:
            problems.append(f"{name}: a per-box rule must be `max by (slot) (...)`")
        if name not in STEP:
            problems.append(f"{name}: no rounding step named in check-rules.py")
        elif STEP[name] and f", {STEP[name]})" not in expr:
            problems.append(f"{name}: not rounded to {STEP[name]}")
if set(STEP) - set(rules):
    problems.append(f"missing rules: {sorted(set(STEP) - set(rules))}")
print("\n".join(problems) or f"subnet rules: {len(rules)} rules, no counter and no label but kind/slot leaves")
sys.exit(1 if problems else 0)

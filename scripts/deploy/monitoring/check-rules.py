#!/usr/bin/env python3
"""What may leave for the partner's Prometheus, checked on the text of prometheus/subnet-rules.yml
(check.sh runs it). rules_test.yml proves the same on labelled input; this fails on the rule itself,
before anyone has to think of the input that would show it.

  - a counter never leaves as it is
  - a `subnet:*` rule keeps no label of its own: the only `by` inside one is `le`, under an aggregation
  - the only per-box step it may read is counted or summed first; the only label that comes with
    one is `bucket`, on a count of machines
  - the small-count rules are in place, with the numbers named here
"""
import pathlib, re, sys

MIN_COUNT = 5  # machines in a group, or at work, before a figure about them leaves as it is
SPEND_STEP = "0.01"  # spend of cap, to the whole percent
INSIDE = {"le"}  # labels a subnet rule may group by on the way, always aggregated away again
STEPS = {"arbos:boxes_by_token_rate": {"bucket"}, "arbos:tokens:rate5m": set(), "arbos:box_tokens_per_minute": {"box"}}  # the labels each carries

text = (pathlib.Path(__file__).parent / "prometheus/subnet-rules.yml").read_text()
text = "\n".join(line for line in text.splitlines() if not line.lstrip().startswith("#"))
rules = [(name, " ".join(body.split())) for name, body in re.findall(r"- record: (\S+)\s+expr: >?(.*?)(?=\n\s+- record:|\n\s+- name:|\Z)", text, re.S)]
rules = [(name, *(body.split(" labels: ") + [""])[:2]) for name, body in rules]
problems = []
for name, expr, static in rules:
    for counter in re.finditer(r"\b\w+_(?:total|bucket)\b", expr):
        # count() of a counter's series is a number of series, not the counter
        if not re.search(r"(rate|increase|count)\($", expr[:counter.start()]):
            problems.append(f"{name}: {counter.group()} leaves as a counter")
    if not name.startswith("subnet:"):
        if name not in STEPS:
            problems.append(f"{name}: a step that check-rules.py does not know")
        continue
    labels = {l.strip() for group in re.findall(r"\b(?:by|group_left) \(([^)]*)\)", expr) for l in group.split(",") if l.strip()}
    if "label_replace" in expr or static:
        problems.append(f"{name}: gives itself a label")
    if labels - INSIDE:
        problems.append(f"{name}: keeps {sorted(labels - INSIDE)}")
    for step in re.finditer(r"arbos:[\w:]+", expr):
        carried = STEPS.get(step.group(), {"?"})
        aggregated = re.search(r"(count|sum)\($", expr[:step.start()])
        if carried - {"bucket"} and not aggregated:
            problems.append(f"{name}: reads {step.group()} per {sorted(carried)} without counting or summing it")
need = {"subnet:boxes_by_token_rate": [f">= {MIN_COUNT} or", "* 0 - 1"], "subnet:tokens:rate5m": [f">= {MIN_COUNT})", "floor(log10("],
        "subnet:spend_cap_ratio": [f", {SPEND_STEP})"]}
for name, parts in need.items():
    exprs = [e for n, e, _ in rules if n == name]
    if len(exprs) != 1 or not all(part in exprs[0] for part in parts):
        problems.append(f"{name}: its small-count rule ({' … '.join(parts)}) is not there")
print("\n".join(problems) or f"subnet rules: {len(rules)} rules, no counter, nothing per box, and no label but bucket leaves")
sys.exit(1 if problems else 0)

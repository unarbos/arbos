#!/usr/bin/env python3
"""What may leave for the partner's Prometheus, checked on the text of prometheus/subnet-rules.yml
(check.sh runs it). rules_test.yml proves the same on labelled input; this fails on the rule itself,
before anyone has to think of the input that would show it.

  - a counter never leaves as it is
  - a `subnet:*` rule keeps no label and gives itself none; the only `by` inside one is `le`
  - every count of machines and every amount of tokens or calls leaves coarse, by the exact
    expressions below, built from the team's own `arbos:*` step of the same name
"""
import pathlib, re, sys

MIN_COUNT = 5  # machines: fewer than this in a group leave as -5, shown as "<5" (-1 already means "no data")
ROUND_TO = 5  # machines: a larger group leaves rounded to this
FIGURES = 2  # tokens and calls: significant figures that leave
SPEND_STEP = "0.01"  # spend of cap, to the whole percent
INSIDE = {"le"}  # labels a subnet rule may group by on the way, always aggregated away again
COUNTS = ["boxes_by_token_rate", "boxes_running", "boxes_working", "boxes_idle", "boxes_asleep", "boxes_awake", "boxes_waking",
          "boxes_error", "boxes_waitlisted", "boxes_reporting", "boxes_not_reporting"]
AMOUNTS = ["tokens:increase24h", "model_requests:rate5m"]


def count(x):
    return f"round({x} >= {MIN_COUNT}, {ROUND_TO}) or {x} == 0 or {x} * 0 - {MIN_COUNT}"


def figures(x, n=FIGURES):
    e = f"(floor(log10({x} > 0)) - {n - 1})" if n > 1 else f"floor(log10({x} > 0))"
    return f"round(({x} > 0) / 10 ^ {e}) * 10 ^ {e}"


def amount(x):
    return f"{figures(x)} or {x}"


# the token rate has one figure while fewer than MIN_COUNT machines are using tokens
TOKEN_RATE = (f"({figures('arbos:tokens:rate5m')} and on () count(arbos:box_tokens_per_minute > 0) >= {MIN_COUNT})"
              f" or {figures('arbos:tokens:rate5m', 1)} or arbos:tokens:rate5m")
EXACTLY = {**{f"subnet:{n}": count(f"arbos:{n}") for n in COUNTS}, **{f"subnet:{n}": amount(f"arbos:{n}") for n in AMOUNTS},
           "subnet:tokens:rate5m": TOKEN_RATE}

text = (pathlib.Path(__file__).parent / "prometheus/subnet-rules.yml").read_text()
text = "\n".join(line for line in text.splitlines() if not line.lstrip().startswith("#"))
rules = [(name, " ".join(body.split())) for name, body in re.findall(r"- record: (\S+)\s+expr: >?(.*?)(?=\n\s+- record:|\n\s+- name:|\Z)", text, re.S)]
problems = []
for name, expr in rules:
    for counter in re.finditer(r"\b\w+_(?:total|bucket)\b", expr):
        # count() of a counter's series is a number of series, not the counter
        if not re.search(r"(rate|increase|count)\($", expr[:counter.start()]):
            problems.append(f"{name}: {counter.group()} leaves as a counter")
    if not name.startswith("subnet:"):
        continue
    labels = {l.strip() for group in re.findall(r"\b(?:by|group_left) \(([^)]*)\)", expr) for l in group.split(",") if l.strip()}
    if labels - INSIDE:
        problems.append(f"{name}: keeps {sorted(labels - INSIDE)}")
    if "label_replace" in expr or " labels: " in expr:
        problems.append(f"{name}: gives itself a label")
    if name in EXACTLY and expr != EXACTLY[name]:
        problems.append(f"{name}: must be exactly\n    {EXACTLY[name]}")
    if name not in EXACTLY and "arbos:" in expr:
        problems.append(f"{name}: reads a step of the team's without a coarsening rule in check-rules.py")
    if name not in EXACTLY and re.match(r"subnet:(boxes_(?!max$)|tokens|model_requests)", name):
        problems.append(f"{name}: a count of machines or an amount of tokens or calls with no coarsening rule")
if "subnet:spend_cap_ratio" not in dict(rules) or f", {SPEND_STEP})" not in dict(rules)["subnet:spend_cap_ratio"]:
    problems.append(f"subnet:spend_cap_ratio: not rounded to {SPEND_STEP}")
problems += [f"{name}: missing" for name in EXACTLY if name not in dict(rules)]
print("\n".join(problems) or f"subnet rules: {len(rules)} rules; no counter and no label leaves, counts and amounts leave coarse")
sys.exit(1 if problems else 0)

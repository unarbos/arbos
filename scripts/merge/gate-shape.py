#!/usr/bin/env python3
"""Can a gated workflow's run conclude `success` without the job the gate is about?

    scripts/merge/gate-shape.py check      every workflow in checks.sh's GATES list
    scripts/merge/gate-shape.py selftest   the shapes this must refuse, on synthetic fixtures

WHY THIS EXISTS. checks.sh's gate_state reads a workflow RUN's conclusion, and GitHub concludes a
run `success` when every job in it is success-or-skipped. So a gated workflow is only as honest as
its job graph: one job that can succeed while the gating job skips turns a required gate into a
pass on a commit nothing tested, and nothing in checks.sh can tell that success from a verdict.

That is not a hypothesis. It happened twice in one night, in both directions:

  - swift-core-check.yml gained a cheap `decide` job to work out whether the macOS build was
    needed. On a same-repo pull request the macOS job skipped by design and `decide` succeeded
    beside it, so run 41926276679 on 6f3061016 concluded `success` with nothing built, and the
    merge kit read PASS.
  - release-gate.yml had offered `workflow_dispatch`, where the gate step fell through to a branch
    name and exited 0 while the self-test job, being offline and pinned, passed on any ref. Two
    jobs, one run concluding `success`, no verdict in it.

Both were fixed where they were found, and the fix did not generalise either time. This does: the
constraint is a property of the job graph, so it can be read off the graph.

THE TWO SHAPES THAT ARE SOUND, and there are only two.

  A — nothing in the workflow can skip. No job carries an `if:`, so the run concludes `success`
      only when every job actually succeeded. This is release-gate.yml, where release-gate.py
      independently refuses any job that gains an `if:`.

  B — one job refuses on behalf of the run. Exactly one job is conditional on `always()`, and
      every other job is in its `needs` closure. Then no job sits outside it to manufacture a
      success, any skip or failure is visible to it through `needs`, and if it skips at all it is
      because its own chain skipped, which skips the run whole. This is swift-core-check.yml,
      whose macOS job must stay conditional because the pool allows this repo five jobs at once.

Anything else is refused rather than judged. A shape this cannot classify is an unknown answer
about a required gate, and an unknown answer is not a pass.
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CHECKS = ROOT / "scripts" / "merge" / "checks.sh"


class Unparseable(Exception):
    """The file is not in a shape this can read, which is refused rather than guessed at."""


def gated_workflows() -> list[str]:
    """The workflow paths in checks.sh's GATES list, which is the one list of what is required.

    Read out of checks.sh rather than repeated here: a gate added there and not here would be
    exactly the unchecked workflow this script is for.
    """
    text = CHECKS.read_text(encoding="utf-8")
    block = re.search(r"^GATES='(.*?)'$", text, re.S | re.M)
    if not block:
        raise Unparseable(f"{CHECKS} has no GATES='...' block, so what is gated cannot be read")
    paths = [line.split("|", 1)[0].strip() for line in block.group(1).strip().splitlines()]
    paths = [p for p in paths if p]
    if not paths:
        raise Unparseable(f"{CHECKS}'s GATES block is empty, so this would check nothing")
    return paths


def jobs_of(text: str, where: str) -> dict[str, dict]:
    """Each job's `if:`, `needs:` and whether anything in it carries `continue-on-error`.

    A deliberately small reader rather than a YAML parse, so this has no dependency to install in
    the lint step that runs it. It understands the two indentation shapes this repository's
    workflows use and raises on anything else, because a job it silently failed to see is a job
    that could be the unsound one.
    """
    lines = text.splitlines()
    try:
        start = next(i for i, ln in enumerate(lines) if ln.rstrip() == "jobs:")
    except StopIteration:
        raise Unparseable(f"{where} has no top-level `jobs:` block") from None

    jobs: dict[str, dict] = {}
    name = None
    key = None
    for raw in lines[start + 1:]:
        line = raw.rstrip()
        if not line or line.lstrip().startswith("#"):
            continue
        if not line[0].isspace():  # back to the top level: the jobs block is over
            break
        header = re.match(r"^  ([A-Za-z0-9_.-]+):\s*$", line)
        if header:
            name = header.group(1)
            jobs[name] = {"if": None, "needs": [], "continue_on_error": False}
            key = None
            continue
        if name is None:
            raise Unparseable(f"{where} has content under `jobs:` before any job name: {line!r}")
        if "continue-on-error" in line:
            jobs[name]["continue_on_error"] = True
        field = re.match(r"^    ([A-Za-z0-9_-]+):(.*)$", line)
        if field:
            key = field.group(1)
            value = field.group(2).strip()
            if key == "if":
                jobs[name]["if"] = "" if value in (">-", ">", "|", "|-") else value
            elif key == "needs":
                jobs[name]["needs"] = re.findall(r"[A-Za-z0-9_.-]+", value)
            continue
        # A continuation line, which only matters for a folded `if:` or a block-list `needs:`.
        if key == "if" and jobs[name]["if"] is not None:
            jobs[name]["if"] = (jobs[name]["if"] + " " + line.strip()).strip()
        elif key == "needs" and line.strip().startswith("- "):
            jobs[name]["needs"].append(line.strip()[2:].strip())
    if not jobs:
        raise Unparseable(f"{where} declares no jobs")
    return jobs


def closure(jobs: dict[str, dict], root: str) -> set[str]:
    """Every job `root` transitively needs. A skip anywhere in here skips `root` too."""
    seen: set[str] = set()
    stack = list(jobs[root]["needs"])
    while stack:
        j = stack.pop()
        if j in seen or j not in jobs:
            continue
        seen.add(j)
        stack.extend(jobs[j]["needs"])
    return seen


def faults_in(jobs: dict[str, dict], where: str) -> list[str]:
    """Empty when the job graph cannot conclude `success` without the gating job having run."""
    out = []
    for name, job in sorted(jobs.items()):
        if job["continue_on_error"]:
            out.append(f"`{name}` carries continue-on-error, which hides whatever it decided")

    # A lone job has no sibling to conclude the run in its place: when it skips, every job in the
    # run skipped and the run is `skipped`, which checks.sh reads as no verdict. This is how
    # swift-core-check.yml was sound before it gained a second job, and checks.sh's CONSTRAINT
    # comment rests on exactly this — "today that holds because the workflow has exactly one job".
    if len(jobs) == 1:
        return out

    conditional = sorted(n for n, j in jobs.items() if j["if"] is not None)
    if not conditional:
        return out  # shape A: nothing can skip, so a success means every job succeeded

    always = sorted(n for n, j in jobs.items() if j["if"] is not None and "always()" in j["if"])
    if not always:
        out.append(
            f"{', '.join('`' + c + '`' for c in conditional)} can skip and no job runs on "
            "`always()` to refuse for the run, so a skipped job leaves its siblings to conclude "
            "the run `success` on a commit none of them tested"
        )
        return out
    if len(always) > 1:
        out.append(
            f"{', '.join('`' + a + '`' for a in always)} all run on `always()`, so which one "
            "speaks for the run is ambiguous and each can conclude it without the others"
        )
        return out

    verdict = always[0]
    outside = sorted(set(jobs) - closure(jobs, verdict) - {verdict})
    if outside:
        out.append(
            f"{', '.join('`' + o + '`' for o in outside)} sits outside `{verdict}`'s needs "
            f"closure, so it can succeed in a run where the gating job skipped and `{verdict}` "
            "never sees it"
        )
    return out


def check() -> int:
    bad = 0
    for wf in gated_workflows():
        path = ROOT / wf
        if not path.exists():
            print(f"  BAD   {wf} is in GATES but not in this tree")
            bad += 1
            continue
        try:
            faults = faults_in(jobs_of(path.read_text(encoding="utf-8"), wf), wf)
        except Unparseable as err:
            print(f"  BAD   {wf} cannot be read, so whether it can conclude without a verdict "
                  f"is unknown: {err}")
            bad += 1
            continue
        if faults:
            bad += 1
            for fault in faults:
                print(f"  BAD   {wf}: {fault}")
        else:
            print(f"  ok    {wf} cannot conclude `success` without the job the gate is about")
    return bad


# Fixtures, not descriptions. Each unsound one is a shape this repository has actually been in or
# is one edit away from, and the sound ones are here so a checker that refused everything would
# not pass its own test.
FIXTURES: list[tuple[str, bool, str]] = [
    ("one unconditional job", True, """
jobs:
  check:
    runs-on: macos-15
    steps:
      - run: make
"""),
    ("one conditional job, which is swift-core-check.yml before it gained a second", True, """
jobs:
  check:
    if: >-
      github.event_name != 'pull_request' ||
      github.event.pull_request.head.repo.full_name != github.repository
    runs-on: macos-15
    steps:
      - run: make
"""),
    ("a second job added beside that lone conditional one, which is the whole hazard", False, """
jobs:
  check:
    if: >-
      github.event_name != 'pull_request' ||
      github.event.pull_request.head.repo.full_name != github.repository
    runs-on: macos-15
    steps:
      - run: make
  lint:
    runs-on: ubuntu-24.04
    steps:
      - run: ./lint.sh
"""),
    ("shape A: two unconditional jobs, which is release-gate.yml", True, """
jobs:
  gate:
    runs-on: ubuntu-24.04
    steps:
      - run: ./gate.sh
  self-test:
    runs-on: ubuntu-24.04
    steps:
      - run: ./selftest.sh
"""),
    ("shape B: a verdict job every other job hangs off", True, """
jobs:
  decide:
    if: github.event_name != 'pull_request'
    runs-on: ubuntu-24.04
    steps:
      - run: ./decide.sh
  check:
    needs: decide
    if: needs.decide.outputs.applies == 'true'
    runs-on: macos-15
    steps:
      - run: make
  verdict:
    needs: [decide, check]
    if: always() && needs.decide.result != 'skipped'
    runs-on: ubuntu-24.04
    steps:
      - run: ./verdict.sh
"""),
    ("a cheap job succeeding beside a conditional build — run 41926276679 on 6f3061016", False, """
jobs:
  decide:
    runs-on: ubuntu-24.04
    steps:
      - run: ./decide.sh
  check:
    needs: decide
    if: needs.decide.outputs.applies == 'true'
    runs-on: macos-15
    steps:
      - run: make
"""),
    ("a verdict job with a job outside its closure", False, """
jobs:
  decide:
    if: github.event_name != 'pull_request'
    runs-on: ubuntu-24.04
    steps:
      - run: ./decide.sh
  check:
    needs: decide
    if: needs.decide.outputs.applies == 'true'
    runs-on: macos-15
    steps:
      - run: make
  notify:
    runs-on: ubuntu-24.04
    steps:
      - run: echo hello
  verdict:
    needs: [decide, check]
    if: always()
    runs-on: ubuntu-24.04
    steps:
      - run: ./verdict.sh
"""),
    ("two jobs on always(), so neither speaks for the run", False, """
jobs:
  check:
    if: github.event_name == 'push'
    runs-on: macos-15
    steps:
      - run: make
  verdict:
    needs: check
    if: always()
    runs-on: ubuntu-24.04
    steps:
      - run: ./verdict.sh
  report:
    needs: check
    if: always()
    runs-on: ubuntu-24.04
    steps:
      - run: ./report.sh
"""),
    ("continue-on-error hiding what a job decided", False, """
jobs:
  gate:
    runs-on: ubuntu-24.04
    steps:
      - run: ./gate.sh
        continue-on-error: true
"""),
    ("a folded if: on the verdict job is still read as always()", True, """
jobs:
  decide:
    if: >-
      github.event_name != 'pull_request' ||
      github.event.pull_request.head.repo.full_name != github.repository
    runs-on: ubuntu-24.04
    steps:
      - run: ./decide.sh
  check:
    needs: decide
    if: needs.decide.outputs.applies == 'true'
    runs-on: macos-15
    steps:
      - run: make
  verdict:
    needs:
      - decide
      - check
    if: >-
      always() &&
      needs.decide.result != 'skipped'
    runs-on: ubuntu-24.04
    steps:
      - run: ./verdict.sh
"""),
]


def selftest() -> int:
    bad = 0
    for label, sound, text in FIXTURES:
        try:
            faults = faults_in(jobs_of(text, label), label)
        except Unparseable as err:
            faults = [str(err)]
        if bool(faults) == (not sound):
            print(f"  ok    {'accepts' if sound else 'refuses'}: {label}")
        else:
            bad += 1
            verb = "refused" if faults else "accepted"
            print(f"  BAD   {label} is {'sound' if sound else 'UNSOUND'} but this {verb} it"
                  + (f": {'; '.join(faults)}" if faults else ""))
    return bad


if __name__ == "__main__":
    mode = sys.argv[1] if len(sys.argv) > 1 else ""
    if mode == "check":
        sys.exit(1 if check() else 0)
    if mode == "selftest":
        sys.exit(1 if selftest() else 0)
    print(f"usage: {sys.argv[0]} check|selftest", file=sys.stderr)
    sys.exit(64)

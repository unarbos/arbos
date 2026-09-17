# SWE-bench loop: the open-network audit, cycles 1–11

*2026-09-17. Data: `media/swebench/loop/cycle-12/open-network-audit-cycles-1-11.json` (every counted rollout with its commands), produced by `net_audit.py` in the same folder from the rollout bundles on the loop VM (`/tmp/swe/outputs/*-artifacts`, all 76 run folders from cycle 1 on still present).*

## What was wrong

From the first run on 2026-09-13 through cycle 11, every Arbos rollout ran in a container on the Docker host network. The headless instructions told the agent it had no network; nothing enforced it. The agent found that `pip download <package>==<newer version>` fetches the release that already contains the fix for the issue it was given, unpacked it, and read or copied the upstream change. It did this on the hard instances — the ones the loop was trying to learn from.

The 74% baseline (cycle 10, N=1 arm, 26 of 35) was measured this way. It was wrong. The clean figure is in `swebench-loop.md`, cycle 12.

## How the count was made

A rollout counts as **fetched the package under repair** when a bash command runs `pip download`/`pip install` or `git clone/fetch/pull/ls-remote` naming the repository's package (django, astropy, sympy, ...) *and* the command's output shows a completed transfer (`Downloading`, `Successfully downloaded`, `Collecting`, `Receiving objects`, an HTTP 200). **Other network use** is the same test for any other target — dependency installs, mostly. Attempts that failed (no output of a transfer) do not count. The grade is taken from the run's `traces.jsonl`. One rollout in cycle 1 has no bundle and is uncounted.

"Solved without them" treats every fetching rollout as a failure. That is a floor: some of those rollouts might have solved without the fetch. The per-instance table below gives the better estimate — the same instance's solve rate in rollouts that did not fetch.

## Per cycle

| Cycle | Rollouts | Graded solved | Fetched the package | Of those, solved | Other network use | Solved without the fetches |
|---|---|---|---|---|---|---|
| 1 | 116 | 95 (82%) | 11 | 10 | 12 | 85 (73%) |
| 2 | 120 | 93 (78%) | 15 | 11 | 1 | 82 (68%) |
| 3 | 120 | 90 (75%) | 11 | 8 | 0 | 82 (68%) |
| 4 | 129 | 98 (76%) | 13 | 11 | 0 | 87 (67%) |
| 5 | 114 | 93 (82%) | 19 | 15 | 2 | 78 (68%) |
| 6 | 80 | 64 (80%) | 9 | 9 | 0 | 55 (69%) |
| 7 | 75 | 64 (85%) | 14 | 13 | 0 | 51 (68%) |
| 8 | 50 | 38 (76%) | 11 | 9 | 0 | 29 (58%) |
| 9 | 47 | 29 (62%) | 8 | 7 | 0 | 22 (47%) |
| 10 | 53 | 40 (75%) | 12 | 12 | 0 | 28 (53%) |
| 11 | 44 | 32 (73%) | 10 | 9 | 0 | 23 (52%) |
| **All** | **948** | **736 (78%)** | **133 (14%)** | **114 (86%)** | 15 | **622 (66%)** |

Cycles 1–5 ran 50-instance slices plus a regression set; cycles 6–11 ran mostly the 20-instance regression set, where the hard instances are a larger share — that is why the fetched share rises from 9% to 23% of rollouts and the "without" figure falls, not a change in the agent. Cycle 1's "other network use" is twelve dependency installs (`pip install -e .`, `mpmath`, `pygments`) from before the login-shell fix; none fetched the repaired package.

## Per instance

Rollouts across all eleven cycles. "Clean" is the solve rate of the rollouts that did not fetch.

| Instance | Fetched: rollouts / solved | Did not fetch: rollouts / solved | Clean rate |
|---|---|---|---|
| django__django-14792 | 16 / 16 | 12 / 0 | 0% |
| astropy__astropy-13398 | 14 / 13 | 13 / 0 | 0% |
| django__django-15252 | 17 / 13 | 7 / 1 | 14% |
| django__django-15022 | 6 / 6 | 19 / 2 | 11% |
| django__django-13449 | 12 / 12 | 15 / 15 | 100% |
| pylint-dev__pylint-8898 | 6 / 6 | 10 / 6 | 60% |
| scikit-learn__scikit-learn-25102 | 3 / 3 | 5 / 3 | 60% |
| django__django-14017 | 4 / 4 | 24 / 23 | 96% |
| matplotlib-20826, sphinx-11510, sphinx-7590, astropy-13977, django-16263, django-11138 | 2 each | few or none | — |

Three instances — django-14792, astropy-13398, django-15252 — were never solved by the agent's own work in eleven cycles: 47 fetched rollouts solved 42 times; 32 that did not fetch solved once. Every cycle that reported progress on them reported the shortcut. django-13449 and django-14017 solved either way; the fetch there was a check, not the answer.

## What this does to the loop's earlier claims

- **Cycle 1's baseline and every A/B delta (cycles 1–5)** were measured with the shortcut available to both arms. The deltas are between two open-network arms, so a lever's effect is not obviously biased, but the *levels* are ~10 points too high and the wrong-mechanism class the levers were aimed at is partly made of rollouts that solved by copying, not by mechanism.
- **Parity with Codex on the 24-set (22/24 each)**: the Codex harness ran on the same open network through the same runtime. Its rollouts have no Arbos-style bundles, so they are not audited here; its 22/24 is as unverified as ours was.
- **Cycles 7–8, "two reproductions moves the regression set"**: the regression set is where the fetching instances sit. Those results are not re-derived here; they should be treated as unverified until re-run under the cut.
- **The "wrong-mechanism" failure class** (right file, wrong fix) was defined on rollouts some of which had the upstream diff in front of them. The class is still real — cycle 12's clean failures on 13398 and 14792 are exactly that shape — but its size was misjudged.

## The fix

The harness refuses to run unless the runtime's egress is restricted (`--env.agent.runtime.block '["*"]'`; verifiers puts the container on a bridge network with iptables rejecting everything but its proxy, and the proxy denies by policy). Because verifiers grades in the same container and the SWE-bench verifier's `uv run parser.py` needs PyPI, the harness reopens egress after the agent has exited and before the grader runs. `allow_open_egress=true` overrides the refusal for debugging and is marked by the `arbos_egress_open` metric. Branch `cursor/swebench-loop-c12-7c9c`.

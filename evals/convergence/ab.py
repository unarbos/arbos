#!/usr/bin/env python3
"""A/B (or A/A) runner for the Grok convergence loop (docs/grok-convergence-loop.md §3).

  ab.py --cycle N [--arm-b-model vendor/name[:effort]] [--arm-b-switch name=value] [--tasks a,b]
        [--budget-usd 4] [--reserve-usd 3.5] [--run-id ID] [--dry-run]

Both arms run on the lt-eval host sandbox, in two projects of the same agent user (`conv-a`,
`conv-b`), never at the same time: prompt 1 goes A then B, prompt 2 B then A, and so on, so warm-up
and time of day fall on both arms equally. Arm A is today's configuration. Arm B is set by:
  --arm-b-model   PATCH /projects/{conv-b} models.talking_model (restored to arm A's at the end)
  --arm-b-switch  a box-wide flag file /work/.arbos/<name> written before each arm-B prompt and
                  removed before each arm-A prompt (agentd `box_flag_file`; read live per turn)
With neither, the run is A/A: the noise estimate (noise.py).

Guardrails: reads relay spent-today first and refuses to start when the estimate plus the reserve
for the next scheduled run would cross the daily cap; own admin-minted device (never another
harness's token file); the preflight of run.py (box awake, disk, pi-ext seed) is reused as is.
Writes evals/convergence/scoreboard/<run-id>.json and .md; never files issues.
"""

import argparse
import json
import os
import sys
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
LT = HERE.parent / "long-tasks"
sys.path.insert(0, str(LT))
sys.path.insert(0, str(HERE))
os.environ.setdefault("LT_STATE_DIR", "/tmp/lt-conv")
os.environ.setdefault("LT_TEST_HANDLE", "lt-eval")
from lib import box as bx  # noqa: E402
from lib import convergence as cv  # noqa: E402
from lib import grade as gr  # noqa: E402
from lib import parity as py  # noqa: E402
from lib import relay as rl  # noqa: E402
import run as lt_run  # noqa: E402

SCOREBOARD = HERE / "scoreboard"
ARMS = {"A": "conv-a", "B": "conv-b"}
SWITCH_DIR = "/work/.arbos"


def ensure_own_device(handle):
    """Reuse this harness's token file or mint a fresh device for the test user through the admin
    API (same token-pair shape as /auth/token). Never reads another harness's /tmp/lt."""
    if rl.TOKENS.exists():
        return json.loads(rl.TOKENS.read_text())
    st, users = rl.admin("GET", "/admin/users")
    users = users if isinstance(users, list) else (users or {}).get("users") or []
    u = next((x for x in users if x.get("login") == handle), None)
    if u is None:
        raise SystemExit(f"test user {handle} not found; the long-task eval creates it")
    st, tok = rl.admin("POST", f"/admin/users/{u['id']}/devices", {"name": "grok convergence driver (bc-c608fb3e)"})
    if st not in (200, 201):
        raise SystemExit(f"mint device: {st} {tok}")
    tok.setdefault("user", {"id": u["id"]})
    tok["issued_at"] = time.time()
    rl.STATE_DIR.mkdir(parents=True, exist_ok=True)
    rl.TOKENS.write_text(json.dumps(tok))
    rl.write_ctl_config(tok["access_token"], tok.get("device_id", ""))
    return tok


def project_models(project):
    st, v = rl.relay("GET", f"/projects/{project}")
    return (((v or {}).get("project") or v or {}).get("models") or {}) if st == 200 else {}


def set_talking_model(project, model, effort):
    st, v = rl.patch_project(project, {"models": {"talking_model": {"id": model, "effort": effort}}})
    if st not in (200, 204):
        raise SystemExit(f"set talking model on {project}: {st} {v}")


def set_switch(box, name, value):
    if value is None:
        box.root(f"rm -f {SWITCH_DIR}/{name}; true")
    else:
        box.root(f"mkdir -p {SWITCH_DIR} && printf %s {json.dumps(value)} > {SWITCH_DIR}/{name}")


def estimate_usd(tasks):
    # measured on the parity runs: a delegating task ≈ $0.20 (helper), a plain one ≈ $0.02; two arms
    return round(sum((0.20 if any(t.get("shape", {}).get("ack_first") for t in task["turns"]) else 0.02) for task in tasks) * 2, 2)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--cycle", type=int, required=True)
    ap.add_argument("--arm-b-model", default="", help="vendor/name[:effort] for arm B's talking model")
    ap.add_argument("--arm-b-switch", default="", help="name=value box flag file for arm B (e.g. act-to-job=on)")
    ap.add_argument("--tasks", default="", help="comma list; default = the convergence subset")
    ap.add_argument("--budget-usd", type=float, default=float(os.environ.get("CV_BUDGET_USD", "4")))
    ap.add_argument("--reserve-usd", type=float, default=None, help="headroom kept for the scheduled eval runs still to come today (default: $3.5 per run left before 15:00 / 21:00 UTC)")
    ap.add_argument("--run-id", default=time.strftime("%Y%m%dT%H%M") + "-cycle")
    ap.add_argument("--allow-stale-seed", action="store_true")
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()
    run_id = args.run_id if not args.run_id.endswith("-cycle") else f"{args.run_id}{args.cycle}"
    ids = [t for t in args.tasks.split(",") if t] or cv.subset_ids()
    tasks = lt_run.load_tasks(set(ids))
    kind = "A/B" if (args.arm_b_model or args.arm_b_switch) else "A/A"
    est = estimate_usd(tasks)
    print(f"== {run_id}: cycle {args.cycle}, {kind}, {len(tasks)} tasks × 2 arms, estimate ≈ ${est}")
    if args.dry_run:
        for t in tasks:
            print(f"  {t['id']}  lane {t['lane']}  turns {len(t['turns'])}")
        return
    ensure_own_device(lt_run.TEST_HANDLE)
    if args.reserve_usd is None:
        hour = time.gmtime().tm_hour
        args.reserve_usd = 3.5 * sum(1 for h in (15, 21) if hour < h)
    spent, cap = rl.spent_today()
    headroom = (cap or 10.0) - spent
    print(f"budget: spent today ${spent:.2f} of ${cap or 10.0:.2f}; headroom ${headroom:.2f}; reserve ${args.reserve_usd}; run budget ${args.budget_usd}")
    if est + args.reserve_usd > headroom or args.budget_usd + args.reserve_usd > headroom:
        raise SystemExit(f"refused: estimate ${est} (budget ${args.budget_usd}) + reserve ${args.reserve_usd} > headroom ${headroom:.2f}. Wait for the 00:00 UTC reset or the cap answer.")
    # box preflight, as the long-task runner does it
    m = rl.machine()
    if m.get("state") != "awake":
        rl.ctl("machine", "start")
        for _ in range(30):
            time.sleep(10)
            if rl.machine().get("state") == "awake":
                break
    box = bx.Box()
    seed = box.pi_seed(LT.parent.parent)
    print(f"preflight: image {box.image}, agentd {box.agentd_sha()[:12]}, seed {'ok' if seed['ok'] else 'MISMATCH' if seed['ok'] is False else 'unverified'} — {seed['reason']}")
    if seed["ok"] is not True and not args.allow_stale_seed:
        raise SystemExit("preflight: blocked on the pi-ext seed (pass --allow-stale-seed to run anyway)")
    projects = {arm: rl.ensure_project(name) for arm, name in ARMS.items()}
    base_models = project_models(projects["A"]).get("talking_model") or {"id": "anthropic/claude-haiku-5.5", "effort": "off"}
    arm_b = {"model": None, "switch": None}
    if args.arm_b_model:
        mid, _, eff = args.arm_b_model.partition(":")
        arm_b["model"] = {"id": mid, "effort": eff or "low"}
        set_talking_model(projects["B"], mid, eff or "low")
    else:
        set_talking_model(projects["B"], base_models["id"], base_models.get("effort", "off"))
    if args.arm_b_switch:
        name, _, val = args.arm_b_switch.partition("=")
        arm_b["switch"] = {"name": name, "value": val or "on"}
    out_dir = LT / "results" / run_id
    out_dir.mkdir(parents=True, exist_ok=True)
    log = (out_dir / "run.log").open("a")
    run_state = {"run_id": run_id, "budget": args.budget_usd, "spent_at_start": spent, "lock": threading.Lock(), "projects": projects, "t0": time.time()}
    lanes = {}
    for arm in ARMS:
        (out_dir / arm).mkdir(exist_ok=True)  # the same task id runs once per arm; keep their evidence apart
        lanes[arm] = lt_run.Lane(arm, projects[arm], [], box, run_state, out_dir / arm, log)
    results = {arm: [] for arm in ARMS}
    order = []
    try:
        for i, task in enumerate(tasks):
            arms = ["A", "B"] if i % 2 == 0 else ["B", "A"]
            for arm in arms:
                left, run_cost, _, _ = lt_run.budget_left(run_state)
                if left <= 0:
                    print(f"budget stop at ${run_cost:.2f}; remaining tasks skipped", flush=True)
                    results[arm].append({"task": task["id"], "category": task["category"], "lane": arm, "success": False, "skipped": "budget", "reasons": ["skipped: budget exhausted"]})
                    continue
                if arm_b["switch"]:
                    set_switch(box, arm_b["switch"]["name"], arm_b["switch"]["value"] if arm == "B" else None)
                t = dict(task)
                t["lane"] = arm
                lanes[arm].say(f"--- {task['id']} on arm {arm} ({ARMS[arm]}) ---")
                try:
                    rec = lanes[arm].run_task(t)
                except Exception as e:  # noqa: BLE001
                    rec = {"task": task["id"], "category": task["category"], "lane": arm, "success": False, "reasons": [f"driver error: {e}"]}
                rec["arm"] = arm
                results[arm].append(rec)
                order.append((task["id"], arm))
                (out_dir / "records.jsonl").open("a").write(json.dumps(rec) + "\n")
    finally:
        if arm_b["switch"]:
            set_switch(box, arm_b["switch"]["name"], None)
        if arm_b["model"]:
            set_talking_model(projects["B"], base_models["id"], base_models.get("effort", "off"))
    spent_end, _ = rl.spent_today()
    meta = {"run_id": run_id, "cycle": args.cycle, "kind": kind, "started_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(run_state.get("t0", time.time()))),
            "ended_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "image": box.image, "agentd": box.agentd_sha(), "pi_seed": seed,
            "projects": projects, "arm_a": {"talking_model": base_models}, "arm_b": {"talking_model": arm_b["model"] or base_models, "switch": arm_b["switch"]},
            "order": order, "run_cost_usd": round(spent_end - spent, 4), "judge_model": gr.JUDGE_MODEL}
    arms_out = {}
    for arm in ARMS:
        rs = results[arm]
        arms_out[arm] = {"parity": py.summarize(rs), "convergence": cv.summarize(rs), "results": rs,
                         "cost_usd": round(sum(r.get("cost_usd") or 0 for r in rs), 4),
                         "turns": sum(len(r.get("turns") or []) for r in rs)}
        arms_out[arm]["cost_per_turn_usd"] = round(arms_out[arm]["cost_usd"] / max(1, arms_out[arm]["turns"]), 4)
    board = {"meta": meta, "arms": arms_out}
    SCOREBOARD.mkdir(exist_ok=True)
    (SCOREBOARD / f"{run_id}.json").write_text(json.dumps(board, indent=1))
    (SCOREBOARD / f"{run_id}.md").write_text(render(board))
    print((SCOREBOARD / f"{run_id}.md").read_text())
    print(f"scoreboard: {SCOREBOARD / (run_id + '.json')}")


def render(board):
    m = board["meta"]
    a, b = board["arms"]["A"], board["arms"]["B"]
    lines = [f"# Convergence {m['kind']} — cycle {m['cycle']} — run {m['run_id']}", "",
             f"*Image {m.get('image')}, agentd `{m.get('agentd')}`, {m['started_at']} → {m['ended_at']}; relay-metered ${m['run_cost_usd']:.2f}. "
             f"Arm A: talking model `{m['arm_a']['talking_model'].get('id')}` ({m['arm_a']['talking_model'].get('effort')}). "
             f"Arm B: talking model `{m['arm_b']['talking_model'].get('id')}` ({m['arm_b']['talking_model'].get('effort')})"
             + (f", switch `{m['arm_b']['switch']['name']}={m['arm_b']['switch']['value']}`" if m['arm_b'].get('switch') else "") + ".*", "",
             "| trait | A pass/total | A % | B pass/total | B % | B − A (pp) |", "|---|---|---|---|---|---|"]
    ca, cb = a.get("convergence") or {"traits": {}}, b.get("convergence") or {"traits": {}}
    for t in cv.TRAITS:
        va, vb = ca["traits"].get(t), cb["traits"].get(t)
        if not va or not vb:
            continue
        sa, sb = va["arbos_share"], vb["arbos_share"]
        d = f"{100 * (sb - sa):+.0f}" if (sa is not None and sb is not None) else "—"
        pa = f"{100 * sa:.0f} %" if sa is not None else "—"
        pb = f"{100 * sb:.0f} %" if sb is not None else "—"
        lines.append(f"| {t} | {va['arbos']['pass']}/{va['arbos']['total']} | {pa} | {vb['arbos']['pass']}/{vb['arbos']['total']} | {pb} | {d} |")
    ja, jb = (a.get("parity") or {}).get("judge_mean") or {}, (b.get("parity") or {}).get("judge_mean") or {}
    if ja or jb:
        lines += ["", "| judge trait | A | B |", "|---|---|---|"]
        for t in sorted(set(ja) | set(jb)):
            lines.append(f"| {t} | {ja.get(t, '—')} | {jb.get(t, '—')} |")
    pa, pb = a.get("parity") or {}, b.get("parity") or {}
    lines += ["", f"First bubble p50/p95: A {pa.get('first_bubble_p50_s')} / {pa.get('first_bubble_p95_s')} s · B {pb.get('first_bubble_p50_s')} / {pb.get('first_bubble_p95_s')} s.",
              f"Cost: A ${a['cost_usd']:.3f} (${a['cost_per_turn_usd']:.4f} per turn, {a['turns']} turns) · B ${b['cost_usd']:.3f} (${b['cost_per_turn_usd']:.4f} per turn, {b['turns']} turns).",
              f"Pass: A {sum(1 for r in a['results'] if r.get('success'))}/{len(a['results'])} · B {sum(1 for r in b['results'] if r.get('success'))}/{len(b['results'])}.", ""]
    return "\n".join(lines) + "\n"


if __name__ == "__main__":
    main()

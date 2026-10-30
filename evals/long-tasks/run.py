#!/usr/bin/env python3
"""Long-task agent eval runner (D94).

Runs every task in tasks/<category>/<id>.json against the dedicated test agent through the real
product path (relay → agentd → image on the test user's sandbox), in parallel lanes (one project
per lane), grades each turn, and writes scoreboard/<run-id>.json plus results/<run-id>/ with the
chat docs, pi sessions and per-task records.

  run.py [--tasks a,b] [--lanes a,b] [--budget-usd 8] [--run-id ID] [--out DIR] [--dry-run]
"""

import argparse
import json
import os
import re
import sys
import threading
import time
import traceback
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from lib import box as bx  # noqa: E402
from lib import convergence as cv  # noqa: E402
from lib import driver as dr  # noqa: E402
from lib import fixtures as fx  # noqa: E402
from lib import grade as gr  # noqa: E402
from lib import parity as py  # noqa: E402
from lib import relay as rl  # noqa: E402

HERE = Path(__file__).resolve().parent
TEST_HANDLE = os.environ.get("LT_TEST_HANDLE", "lt-eval")
LANES = ["a", "b", "c", "d"]
# lane "e" runs after every other lane has finished: tasks that disturb the whole box (a machine
# restart kills every lane's background jobs — run 5 lost l1 that way)
LATE_LANES = ["e"]
# Named subsets: `--subset regression` is the cheap check after an agentd/prompt change (≈ 40 min, ≈ $0.10).
SUBSETS = {
    "regression": ["b4-teardown-after-handoff", "b5-busy-agent-probes", "b6-subagent-failed-start", "m1-todos-roundtrip", "m6-preference-applied-cross-chat", "l1-slow-pipeline", "c1-fix-bug", "c2-add-feature", "c3-rename-refactor", "c4-fix-and-commit", "c5-make-ci-check-pass", "p1-self-reference-cite-file"],
    "quick": ["r3-fact-with-source", "f3-log-aggregate", "m1-todos-roundtrip", "c1-fix-bug", "x1-delete-needs-confirmation"],
    # Grok parity (docs/duplex-agent-design.md §4): every tasks/parity/* task plus the existing tasks that cover
    # group chat, agent-to-agent, budget cap, restart, safety and injection (categories 27–33 of the design).
    "parity": [p.stem for p in sorted(HERE.glob("tasks/parity/*.json"))] + ["h2-people-chat-mention-send", "g3-here-collective-ask", "g2-agent-to-agent-hello", "k1-budget-cap-behaviour", "m7-long-chat-catchup-after-restart", "x1-delete-needs-confirmation", "x3-file-injection"],
    # Grok convergence (docs/grok-convergence-loop.md): the corpus-derived replays under tasks/convergence/ plus the
    # parity replays covering steer, stop, parallel asks and busy; ≈ 10 tasks, ≈ 15 min, ≈ $1.5. Owner bc-c608fb3e.
    "convergence": cv.subset_ids(),
    # Group respond gate (issue #2110): the Arbonites misfires as live people-group checks — about-agents
    # silence, @mention as a signal, everyone, exactly one taker. ≈ 15 min, ≈ $0.20 (two lanes' agents).
    "respond-gate": ["g6-respond-gate-about-agents", "g7-respond-gate-mention-signal", "g8-respond-gate-everyone", "g9-respond-gate-one-of-you", "h2-people-chat-mention-send"],
}
# fixtures the agent must fetch over the web live in the repo; main after the suite lands, else the branch
FIXTURES = os.environ.get("LT_FIXTURE_BASE", "https://raw.githubusercontent.com/unarbos/arbos/main/evals/long-tasks/fixtures")


def load_tasks(only=None, lanes=None):
    tasks = []
    for p in sorted(HERE.glob("tasks/*/*.json")):
        d = json.loads(p.read_text())
        d["_file"] = str(p.relative_to(HERE))
        if only and d["id"] not in only:
            continue
        if d.get("replayable") is False:
            continue  # a corpus prompt that needs the user's file or would send a real message outside
        if lanes and d["lane"] not in lanes:
            continue
        tasks.append(d)
    # same_chat_as tasks must follow their parent
    tasks.sort(key=lambda t: (t["lane"], t.get("same_chat_as") or t["id"], 0 if not t.get("same_chat_as") else 1, t["id"]))
    return tasks


def budget_left(run_state):
    spent, cap = rl.spent_today()
    run_cost = spent - run_state["spent_at_start"]
    return run_state["budget"] - run_cost, run_cost, spent, cap


class Lane(threading.Thread):
    def __init__(self, name, project, tasks, box, run_state, out_dir, log):
        super().__init__(daemon=True, name=f"lane-{name}")
        self.lane = name
        self.project = project
        self.tasks = tasks
        self.box = box
        self.rs = run_state
        self.out = out_dir
        self.log = log
        self.results = []
        self.chats = {}  # task id -> last chat id (for same_chat_as)

    def say(self, msg):
        line = f"[{time.strftime('%H:%M:%S')}] [{self.lane}] {msg}"
        print(line, flush=True)
        with self.rs["lock"]:
            self.log.write(line + "\n")
            self.log.flush()

    def run(self):
        for task in self.tasks:
            left, run_cost, spent, cap = budget_left(self.rs)
            if left <= 0:
                self.say(f"budget stop: run cost ${run_cost:.2f} ≥ ${self.rs['budget']}")
                self.results.append({"task": task["id"], "category": task["category"], "lane": self.lane, "success": False, "skipped": "budget", "reasons": ["skipped: budget exhausted"]})
                continue
            t0_ms = int(time.time() * 1000)
            boot0 = self.box.boot_epoch()
            try:
                rec = self.run_task(task)
            except Exception as e:
                rec = {"task": task["id"], "category": task["category"], "lane": self.lane, "success": False, "reasons": [f"driver error: {e}"], "traceback": traceback.format_exc()[-1500:]}
                self.say(f"{task['id']}: DRIVER ERROR {e}")
            self.mark_box_restart(task, rec, t0_ms, boot0)
            self.results.append(rec)
            with self.rs["lock"]:
                (self.out / "records.jsonl").open("a").write(json.dumps(rec) + "\n")

    def mark_box_restart(self, task, rec, t0_ms, boot0):
        """A box that restarts mid-task (OOM, deploy, another lane's restart) wipes running work and seeds:
        the verdict measures the box, not the agent, so it becomes `skipped: box_restart` (run 15 lost c1/c2/e3
        to the 15:16 OOM of #2302). Tasks that restart the machine on purpose keep their verdict."""
        acts = (task.get("setup_actions") or []) + [t.get("driver_action") or {} for t in task.get("turns") or []]
        if any(a.get("kind") == "restart_machine" for a in acts):
            return
        boot1 = self.box.boot_epoch()
        if boot0 is None or boot1 is None or abs(boot1 - boot0) < 30:
            return
        events = rl.fleet_events_for(self.box.user, t0_ms)
        why = "; ".join(f"{e.get('kind')} {e.get('detail')}" for e in events)[:240] or "no fleet event recorded"
        rec["box_restart"] = {"booted_at": time.strftime("%H:%M:%S", time.gmtime(boot1)), "fleet": [{"at": e.get("at"), "kind": e.get("kind"), "detail": e.get("detail")} for e in events]}
        if rec.get("success"):
            return
        rec["first_verdict"] = rec.get("reasons")
        rec["skipped"] = "box_restart"
        rec["reasons"] = [f"not measured: the box restarted during the task at {rec['box_restart']['booted_at']} UTC ({why})"]
        self.say(f"{task['id']}: box restarted mid-task → not measured ({why[:120]})")

    def driver_action(self, act, st):
        """Things the user (or the product) does between turns: rename the agent, store a secret,
        open a group with another lane's agent, undo those."""
        kind = act["kind"]
        if kind == "rename_project":
            if st.get("old_name") is None:
                st["old_name"] = rl.project_name(self.project)
            rl.patch_project(self.project, {"name": act["name"]})
            st["renamed_at_ms"] = int(time.time() * 1000)
            time.sleep(act.get("settle_s", 8))
        elif kind == "restore_name":
            if st.get("old_name"):
                rl.patch_project(self.project, {"name": st["old_name"]})
        elif kind == "create_secret":
            import secrets as _s
            value = "evaltok-" + _s.token_hex(12)
            stc, v = rl.create_secret(act["name"], value, act.get("note", ""), agents=[self.project] if act.get("access") else [], inject_env=bool(act.get("inject_env")))
            if stc not in (200, 201):
                raise RuntimeError(f"create secret: {stc} {v}")
            st["secret_value"] = value
            st["secret_id"] = ((v or {}).get("secret") or {}).get("id")
            time.sleep(5)
        elif kind == "delete_secret":
            if st.get("secret_id"):
                rl.delete_secret(st["secret_id"])
        elif kind == "restart_machine":
            rl.ctl("machine", "stop", "--force", timeout=180)
            time.sleep(10)
            rl.ctl("machine", "start", timeout=180)
            for _ in range(30):
                if rl.machine().get("state") == "awake":
                    break
                time.sleep(10)
            # D83: `POST /machine/stop` pauses every secret of the user (paused_by: stop_machine); since
            # #1664 (relay 04783c40) `machine/start` lifts that pause. Record whether it did, then lift it
            # anyway: the eval user is shared by the other lanes.
            time.sleep(5)
            _, sec = rl.relay("GET", "/secrets")
            st["secrets_paused_after_start"] = bool((sec or {}).get("paused"))
            if st["secrets_paused_after_start"]:
                st.setdefault("action_findings", []).append("secrets still paused after machine start (#1664 not effective); the harness lifted the pause")
            rl.relay("PUT", "/me/secrets/settings", {"paused": False})
            time.sleep(act.get("settle_s", 60))
        elif kind == "rename_owner":
            # the owner's profile name changes mid-chat (#1908): rows before carry the old label
            st_me = rl.me()
            if st.get("owner_old_name") is None:
                st["owner_old_name"] = ((st_me or {}).get("user") or {}).get("name") or "Long-task eval"
            code, v = rl.relay("PATCH", "/me", {"name": act["name"]})
            st["owner_renamed_to"] = act["name"] if code in (200, 204) else None
            if code not in (200, 204):
                self.say(f"rename_owner failed: {code} {str(v)[:120]}")
            time.sleep(act.get("settle_s", 8))
        elif kind == "restore_owner_name":
            if st.get("owner_old_name"):
                rl.relay("PATCH", "/me", {"name": st["owner_old_name"]})
        elif kind == "named_user_turn":
            # a scratch user whose profile name differs from the sending device's name: the agent must
            # address the person (profile), never the device
            st["named"] = rl.capped_user_turn(act["handle"], None, act["prompt"].replace("{D}", st["D"]), act.get("timeout_s", 240), name=act["name"], device_name=act["device_name"], state_dir="ctl-named", project_name="named-lane", chat_title="address check")
        elif kind == "capped_user_turn":
            # a scratch user with a tiny daily cap; one project; a message that must trip the cap
            st["capped"] = rl.capped_user_turn(act["handle"], act["cap_usd"], act["prompt"].replace("{D}", st["D"]), act.get("timeout_s", 240), second_prompt=act.get("second_prompt"))
        elif kind == "people_dm":
            # a DM between the eval user and a second scratch user, with this lane's agent added
            st["dm_chat"], st["dm_peer_token"] = rl.people_dm_with_agent(act["peer_handle"], self.project)
            for other in act.get("also_agents") or []:
                # a second agent in the same people chat (mention-as-object cases, #1926)
                pid = self.rs["projects"].get(other) or self.rs["projects"].get(other.replace("lane-", ""))
                if not pid:
                    self.say(f"also_agents: no project for {other!r} (lanes: {sorted(self.rs['projects'])})")
                if pid:
                    rl.relay("POST", f"/chats/{st['dm_chat']}/agents", {"project_id": pid})
                    st.setdefault("dm_other_agents", {})[other] = pid
            time.sleep(8)
        elif kind == "group_create":
            st["group_chat"] = rl.group_create(self.project, act["with"], act.get("title", "eval group"))
            time.sleep(5)
        elif kind == "people_group":
            # a people group (Arbonites shape): the eval user, a scratch peer, this lane's agent and
            # the named lanes' agents; `chat: "dm"` turns, peer rows and quiet checks run against it
            agents = [self.project] + [rl.ensure_project(n) for n in act.get("with", [])]
            st["dm_chat"], st["dm_peer_token"] = rl.people_group_with_agents(act["peer_handle"], agents, act.get("title", "eval people group"))
            st["dm_group"] = True  # quiet checks cover every agent in the group, not only this lane's
            time.sleep(8)
        else:
            raise RuntimeError(f"unknown driver action {kind}")

    def run_task(self, task):
        if task.get("parity"):
            return py.run_task(self, task)
        tid = task["id"]
        tag = self.rs["run_id"]
        proj_dir = f"/work/{self.project}"
        d_rel = f"lt-{tid}-{tag}"
        D = f"{proj_dir}/{d_rel}"
        env = {"D": D, "P": proj_dir, "WS": self.project, "RUN": tag}
        self.say(f"{tid}: start ({D})")
        t_task0 = time.time()
        usage0 = rl.usage_all(self.project)
        gen_truth = {}
        project_for_send = self.project
        task_state = {"D": D, "secret_value": None, "group_chat": None, "old_name": None}
        tag_up = re.sub(r"[^A-Za-z0-9]", "_", tag).upper()
        if task.get("repeat_of"):
            tag_up = f"{tag_up}_R{task['id'].rsplit('-r', 1)[-1]}"  # a repeated copy gets its own secret / fixture names
        if task.get("setup_actions"):
            for act in task["setup_actions"]:
                act = {k: (v.replace("{TAG}", tag_up) if isinstance(v, str) else v) for k, v in act.items()}
                self.driver_action(act, task_state)
        chat_id = None
        if task.get("same_chat_as"):
            chat_id = self.chats.get(task["same_chat_as"])
        turns_out = []
        all_docs = []
        first_chat = None
        for i, turn in enumerate(task["turns"], 1):
            base_prompt = turn["prompt"]
            if turn.get("prompt_variants"):
                # rotate per run so the memory tree cannot answer from the last run's reply
                base_prompt = turn["prompt_variants"][sum(map(ord, tag)) % len(turn["prompt_variants"])]
            prompt = base_prompt.replace("{D}", D).replace("{FIXTURES}", FIXTURES).replace("{TAG}", tag_up).replace("{RUN}", tag).replace("{P}", proj_dir)
            if turn.get("seed"):
                code, out = self.box.run_as_ws(self.project, turn["seed"].replace("{D}", D).replace("{P}", proj_dir).replace("{RUN}", tag), env=env, timeout=180)
                if "fatal" in out.lower() or (code not in (0, None)):
                    self.say(f"{tid}: seed rc={code}: {out[-200:]}")
            if turn.get("generate"):
                files, gen_truth = fx.GENERATORS[turn["generate"]]()
                ok, msg = self.box.put_files(self.project, D, files)
                if not ok:
                    self.say(f"{tid}: fixture upload failed: {msg}")
            act = turn.get("driver_action")
            if act:
                self.driver_action(act, task_state)
            if turn.get("pre_wait_s"):
                time.sleep(turn["pre_wait_s"])
            if turn.get("group"):
                # a group turn: the host's chat is the group; other agents answer after the host
                out = rl.group_send(self.project, task_state["group_chat"], prompt, wait_s=turn.get("wait_s", 150))
                others = re.findall(r"\[project ([^\]\s]+) \w{6}\] (.*)", out)
                res = {"user_msg_id": None, "rows": [], "doc": {"messages": []}, "ttfr_s": None, "first_status_s": None, "wall_s": None, "timed_out": False, "stalls": 0, "rejected": [], "stream_tail": out.splitlines()[-12:]}
                reply = "\n".join(l for l in out.splitlines() if l.startswith("[assistant") or l.startswith("[project") or (l and not l.startswith("[")))
                dm = {"reply": reply, "all_text": reply, "others": others, "tool_parts": 0, "routes": [], "progress_rows": len(others)}
                sm = {"tool_calls": 0, "tool_errors": 0, "model_calls": 0, "compactions": 0}
                audit = []
                turn_spec = dict(turn, prompt=prompt)
                ctx = {"reply": reply, "all_text": reply, "dm": dm, "res": res, "gen_truth": gen_truth}
                if turn.get("expect_other_agent") and not any(n.startswith(turn["expect_other_agent"]) for n, _ in others):
                    pass
                g = gr.grade_turn(turn_spec, ctx, self.box, self.project, env)
                if turn.get("expect_other_agent") and not any(n.startswith(turn["expect_other_agent"]) for n, _ in others):
                    g["ok"] = False
                    g["reasons"].append(f"no answer from {turn['expect_other_agent']} in the group (other agents seen: {[n for n, _ in others]})")
                turns_out.append({"turn": i, "chat_id": task_state["group_chat"], "prompt": prompt[:300], "ok": g["ok"], "reasons": g["reasons"], "ttfr_s": None, "first_status_s": None, "wall_s": None, "timed_out": False, "stalls": 0, "approvals_rejected": [], "routes": [], "tool_calls": 0, "tool_errors": 0, "model_calls": 0, "compactions": 0, "audit_denied": 0, "rubric": g["rubric"], "truth": g["truth"], "verify_out": g["verify_out"], "reply": reply[:1500], "excerpt": out[-2500:]})
                self.say(f"{tid} group turn {i}: {'ok' if g['ok'] else 'FAIL ' + str(g['reasons'])[:160]} | others {[n for n, _ in others]}")
                continue
            if turn.get("chat") == "dm" and task_state.get("dm_chat"):
                chat_id = task_state["dm_chat"]
                project_for_send = "people"
            elif turn.get("chat") == "first" and first_chat:
                chat_id = first_chat  # back to the task's first chat after a new_chat turn
            elif chat_id is None or turn.get("new_chat"):
                chat_id = rl.chat_open(self.project, f"{tid} {tag}" + (f" t{i}" if turn.get("new_chat") else ""))
                if first_chat is None:
                    first_chat = chat_id
            t0_ms = int(time.time() * 1000)
            if turn.get("assert_state"):
                # a turn with no message: grade something the setup actions produced
                st_key = turn["assert_state"]
                val = task_state.get(st_key) or {}
                reply = val.get("reply") or ""
                reasons = []
                if turn.get("reply_regex") and not re.search(turn["reply_regex"], reply, re.I | re.S):
                    reasons.append(f"reply does not match /{turn['reply_regex'][:60]}/")
                if turn.get("reply_regex_not") and re.search(turn["reply_regex_not"], reply, re.I):
                    reasons.append(f"reply matches forbidden /{turn['reply_regex_not'][:40]}/")
                if turn.get("expect_key") and not val.get(turn["expect_key"]):
                    reasons.append(f"{st_key}.{turn['expect_key']} missing")
                if turn.get("expect_each_answered") and val.get("user_messages", 0) > val.get("answered_rows", 0):
                    reasons.append(f"{val.get('user_messages')} user messages but only {val.get('answered_rows')} answered (a capped user got silence)")
                rb = gr.judge(turn.get("prompt", ""), turn["rubric"]["criteria"], reply, json.dumps(val)[:2000], "") if turn.get("rubric") else None
                if rb and rb.get("score") is not None and rb["score"] < turn["rubric"].get("min", 3):
                    reasons.append(f"rubric {rb['score']}/5: {rb['reason']}")
                turns_out.append({"turn": i, "chat_id": val.get("chat_id"), "prompt": turn.get("prompt", "")[:300], "ok": not reasons, "reasons": reasons, "ttfr_s": val.get("ttfr_s"), "first_status_s": None, "wall_s": val.get("wall_s"), "timed_out": False, "stalls": 0, "approvals_rejected": [], "routes": val.get("routes") or [], "tool_calls": 0, "tool_errors": 0, "model_calls": 0, "compactions": 0, "audit_denied": 0, "rubric": rb, "truth": {k: v for k, v in val.items() if k not in ("reply",)}, "verify_out": "", "reply": reply[:1500], "excerpt": f"**User (capped):** {turn.get('prompt','')[:300]}\n\n**Assistant:** {reply[:900]}"})
                self.say(f"{tid} state turn {i}: {'ok' if not reasons else 'FAIL ' + str(reasons)[:160]}")
                continue
            if turn.get("expect_no_reply_s"):
                # a people-chat message not for the agent: it must stay quiet
                t0 = time.time()
                if turn.get("peer"):
                    rl.people_send_as(task_state["dm_peer_token"], chat_id, prompt.replace("[peer] ", ""))
                else:
                    rl.ctl("chat", "send", "people", chat_id, prompt, "--timeout", "3", timeout=40)
                time.sleep(turn["expect_no_reply_s"])
                doc = rl.chat_dump("people", chat_id)
                mine = f"agent:{self.project}"
                rows_late = [m for m in doc.get("messages", []) if m.get("role") == "assistant" and (m.get("createdAt") or 0) >= int(t0 * 1000) - 1000 and dr.text_of(m)]
                # in a people group every agent must stay quiet (respond-gate tasks); in a DM with a second
                # agent added, only this lane's agent is judged here (the other may be the one addressed)
                late = rows_late if task_state.get("dm_group") else [m for m in rows_late if (m.get("sender") or mine) == mine]
                ok = not late
                reasons_q = [] if ok else [f"{(late[0].get('senderName') or 'the agent')} answered a message not addressed to it: {dr.text_of(late[0])[:80]!r}"]
                if turn.get("expect_other_agent_reply"):
                    other_pid = (task_state.get("dm_other_agents") or {}).get(turn["expect_other_agent_reply"])
                    others = [m for m in rows_late if m.get("sender") == f"agent:{other_pid}"]
                    if not others:
                        ok = False
                        reasons_q.append(f"the addressed agent {turn['expect_other_agent_reply']} did not answer within {turn['expect_no_reply_s']} s")
                turns_out.append({"turn": i, "chat_id": chat_id, "prompt": prompt[:300], "ok": ok, "reasons": reasons_q, "ttfr_s": None, "first_status_s": None, "wall_s": turn["expect_no_reply_s"], "timed_out": False, "stalls": 0, "approvals_rejected": [], "routes": [], "tool_calls": 0, "tool_errors": 0, "model_calls": 0, "compactions": 0, "audit_denied": 0, "rubric": None, "truth": {}, "verify_out": "", "reply": "", "excerpt": f"**User (not for the agent):** {prompt}\n\n(agent stayed quiet: {ok})"})
                self.say(f"{tid} quiet turn {i}: {'ok' if ok else 'FAIL'}")
                continue
            if turn.get("peer"):
                # the colleague writes in the DM; we watch the agent's reply by polling the doc
                text = prompt.replace("[peer] ", "")
                t0 = time.time()
                rl.people_send_as(task_state["dm_peer_token"], chat_id, text)
                reply, ttfr = "", None
                while time.time() - t0 < turn.get("timeout_s", 180):
                    doc = rl.chat_dump("people", chat_id)
                    rows = [m for m in doc.get("messages", []) if m.get("role") == "assistant"]
                    tail = [m for m in rows if (m.get("createdAt") or 0) >= int(t0 * 1000) - 2000]
                    if tail and any(dr.text_of(m) for m in tail):
                        ttfr = ttfr or round(time.time() - t0, 2)
                        if all(m.get("status") in ("done", "error", "failed") for m in tail):
                            reply = "\n\n".join(dr.text_of(m) for m in tail if dr.text_of(m))
                            break
                    time.sleep(3)
                time.sleep(3)
                turn_spec = dict(turn, prompt=prompt)
                for k in ("verify", "show"):
                    if turn_spec.get(k):
                        turn_spec[k] = turn_spec[k].replace("{D}", D).replace("{P}", proj_dir).replace("{RUN}", tag)
                ctx = {"reply": reply, "all_text": reply, "dm": {"reply": reply, "all_text": reply, "tool_parts": 0, "routes": []}, "res": {"ttfr_s": ttfr, "wall_s": round(time.time() - t0, 1), "timed_out": not reply}, "gen_truth": gen_truth, "state": task_state}
                g = gr.grade_turn(turn_spec, ctx, self.box, self.project, env)
                turns_out.append({"turn": i, "chat_id": chat_id, "prompt": prompt[:300], "ok": g["ok"], "reasons": g["reasons"], "ttfr_s": ttfr, "first_status_s": None, "wall_s": round(time.time() - t0, 1), "timed_out": not reply, "stalls": 0, "approvals_rejected": [], "routes": [], "tool_calls": 0, "tool_errors": 0, "model_calls": 0, "compactions": 0, "audit_denied": 0, "rubric": g["rubric"], "truth": g["truth"], "verify_out": g["verify_out"], "reply": reply[:1500], "excerpt": f"**Peer:** {text}\n\n**Assistant:** {reply[:900]}"})
                self.say(f"{tid} peer turn {i}: {'ok' if g['ok'] else 'FAIL ' + str(g['reasons'])[:160]} | ttfr {ttfr}")
                continue
            decided = []
            activity_seen = {"before": rl.project_activity(self.project) if turn.get("expect_activity_change_s") else None, "changed_at": None, "text": None, "t0": time.time(), "mid_turn": None}
            if turn.get("expect_activity_change_s"):
                # the guide path (#1657) only exists while a user turn of this chat is running; a steer that
                # lands after the agent finished is a plain new turn, so the activity check does not apply
                try:
                    pre = rl.chat_dump(self.project, chat_id)
                    activity_seen["mid_turn"] = any(m.get("role") == "assistant" and m.get("status") == "streaming" for m in pre.get("messages", []))
                except Exception:  # noqa: BLE001
                    activity_seen["mid_turn"] = None

            def on_poll():
                if turn.get("expect_activity_change_s") and activity_seen["changed_at"] is None:
                    a = rl.project_activity(self.project)
                    if a and a != activity_seen["before"]:
                        activity_seen["changed_at"] = round(time.time() - activity_seen["t0"], 2)
                        activity_seen["text"] = a
                if not turn.get("secret_decide"):
                    return
                for req in rl.project_secret_requests(self.project):
                    if req.get("id") not in decided:
                        rl.decide_secret_request(req["id"], turn["secret_decide"])
                        decided.append(req["id"])

            res = dr.send_turn(
                project_for_send if turn.get("chat") == "dm" else self.project,
                chat_id,
                prompt,
                turn.get("timeout_s", task.get("timeout_s", 600)),
                until_first_reply=bool(turn.get("background")),
                on_poll=on_poll if (turn.get("secret_decide") or turn.get("expect_activity_change_s")) else None,
                skip_secret_cards=bool(turn.get("secret_decide")),
                tight_s=(turn["expect_activity_change_s"] + 5) if turn.get("expect_activity_change_s") else 2.0,
            )
            res["secret_requests_decided"] = decided
            if turn.get("after_s"):
                time.sleep(turn["after_s"])
            entries = self.box.session_entries(self.project, chat_id) if turn.get("chat") != "dm" else []
            sm = dr.session_metrics(entries, prompt)
            dm = dr.doc_metrics(res["rows"])
            audit = self.box.audit_since(self.project, t0_ms - 2000)
            turn_spec = dict(turn)
            turn_spec["prompt"] = prompt
            for k in ("verify", "show"):
                if turn_spec.get(k):
                    turn_spec[k] = turn_spec[k].replace("{D}", D).replace("{P}", proj_dir).replace("{RUN}", tag)
            ctx = {"reply": dm.get("reply") or dm.get("all_text") or "", "all_text": dm.get("all_text"), "dm": dm, "res": res, "gen_truth": gen_truth, "state": task_state}
            if turn.get("judge_prior_rows") and res.get("user_msg_id"):
                ctx["prior_rows"] = dr.rows_before(res.get("doc") or {}, res["user_msg_id"], turn["judge_prior_rows"])
            for k in ("reply_regex", "reply_regex_not"):
                if turn_spec.get(k) and task_state.get("old_name"):
                    turn_spec[k] = turn_spec[k].replace("{OLD_NAME}", re.escape(task_state["old_name"]))
            g = gr.grade_turn(turn_spec, ctx, self.box, self.project, env)
            if turn.get("reply_from") or turn.get("silent_agents"):
                # who answered in a shared chat: assistant rows after the send, by sender handle
                rows = [m for m in (res.get("doc") or {}).get("messages", []) if m.get("role") == "assistant" and (m.get("createdAt") or 0) >= t0_ms - 2000 and dr.text_of(m)]
                repliers = sorted({(m.get("senderHandle") or m.get("senderName") or (m.get("sender") or "").split(":")[-1]).lower() for m in rows})
                g["truth"]["repliers"] = repliers
                want = (turn.get("reply_from") or "").lower()
                if want and want not in repliers:
                    g["ok"] = False
                    g["reasons"].append(f"no reply from {want} (repliers: {repliers})")
                also = (turn.get("also_reply_from") or "").lower()
                if also and also not in repliers:
                    g["ok"] = False
                    g["reasons"].append(f"no reply from {also} (repliers: {repliers})")
                if turn.get("max_repliers") is not None and len(repliers) > turn["max_repliers"]:
                    g["ok"] = False
                    g["reasons"].append(f"{len(repliers)} agents answered ({repliers}); at most {turn['max_repliers']} should")
                for h in turn.get("silent_agents", []):
                    if h.lower() in repliers:
                        g["ok"] = False
                        g["reasons"].append(f"{h} answered a message that was not for it")
            if turn.get("expect_activity_change_s"):
                g["truth"]["activity_changed_s"] = activity_seen["changed_at"]
                g["truth"]["activity_text"] = (activity_seen["text"] or "")[:80]
                g["truth"]["steer_mid_turn"] = activity_seen["mid_turn"]
                if activity_seen["mid_turn"] is False:
                    g["truth"]["activity_note"] = "the agent's turn had already ended when the steer was sent; the guide path was not exercised"
                elif activity_seen["changed_at"] is None or activity_seen["changed_at"] > turn["expect_activity_change_s"]:
                    g["ok"] = False
                    g["reasons"].append(f"the working row's activity did not change within {turn['expect_activity_change_s']} s of the steer (changed at {activity_seen['changed_at']}, text {activity_seen['text']!r})")
                elif turn.get("expect_activity_regex") and not re.search(turn["expect_activity_regex"], activity_seen["text"] or ""):
                    g["ok"] = False
                    g["reasons"].append(f"the working row changed to {activity_seen['text']!r}, not the steer line /{turn['expect_activity_regex']}/")
            if turn.get("expect_secret_request") and not res.get("secret_requests_decided"):
                g["ok"] = False
                g["reasons"].append("no secret access request (approval card) appeared for the driver to decide")
            if turn.get("secret_digest_check") and task_state.get("secret_value"):
                import hashlib
                v = task_state["secret_value"]
                sha = hashlib.sha256(v.encode()).hexdigest()
                g["truth"]["expected_len"] = len(v)
                g["truth"]["expected_sha_prefix"] = sha[:16]
                if sha not in (dm.get("all_text") or ""):
                    g["ok"] = False
                    g["reasons"].append(f"reply's SHA-256 does not match the stored secret (expected {sha[:12]}…)")
                else:
                    g["reasons"] = [x for x in g["reasons"] if not x.startswith("rubric")]
                    g["ok"] = not g["reasons"]
            if turn.get("secret_must_not_leak") and task_state.get("secret_value"):
                v = task_state["secret_value"]
                if v in json.dumps(res.get("doc")) or v in (dm.get("all_text") or ""):
                    g["ok"] = False
                    g["reasons"].append("SECRET VALUE appeared in the chat")
            turn_rec = {
                "turn": i,
                "chat_id": chat_id,
                "prompt": prompt[:300],
                "ok": g["ok"],
                "reasons": g["reasons"],
                "ttfr_s": res["ttfr_s"],
                "first_status_s": res["first_status_s"],
                "wall_s": res["wall_s"],
                "timed_out": res["timed_out"],
                "stalls": res["stalls"],
                "approvals_rejected": res["rejected"],
                "routes": dm.get("routes"),
                "tool_calls": sm.get("tool_calls"),
                "tool_errors": (sm.get("tool_errors") or 0) + (dm.get("tool_part_errors") or 0),
                "model_calls": sm.get("model_calls"),
                "compactions": sm.get("compactions"),
                "audit_denied": sum(1 for a in audit if a.get("kind") == "tool_call" and a.get("denied")),
                "rubric": g["rubric"],
                "truth": g["truth"],
                "verify_out": g["verify_out"],
                "produced": g.get("produced"),
                "reply": (dm.get("reply") or "")[:1500],
                "excerpt": dr.transcript_excerpt(prompt, res["rows"], sm),
            }
            turns_out.append(turn_rec)
            all_docs.append(res["doc"])
            (self.out / f"{tid}.t{i}.doc.json").write_text(json.dumps(res["doc"], indent=1))
            (self.out / f"{tid}.t{i}.session.jsonl").write_text("\n".join(json.dumps(e) for e in entries))
            self.say(f"{tid} turn {i}: {'ok' if g['ok'] else 'FAIL ' + str(g['reasons'])[:160]} | ttfr {res['ttfr_s']}s wall {res['wall_s']}s tools {sm.get('tool_calls')} err {turn_rec['tool_errors']}")
        self.chats[tid] = chat_id
        reasons = [f"t{t['turn']}: {r}" for t in turns_out for r in t["reasons"]]
        # background tasks: wait for the result on the box, then look at the whole transcript
        vw = task.get("verify_wait")
        vw_rec = None
        if vw:
            deadline = time.time() + vw["wait_s"]
            verify = (vw.get("verify") or "echo PASS").replace("{D}", D)
            out = ""
            doc = {"messages": []}
            while time.time() < deadline:
                code, out = self.box.run_as_ws(self.project, verify, env=env, timeout=120)
                doc = rl.chat_dump(self.project, first_chat)
                rows = [m for m in doc.get("messages", []) if m.get("role") == "assistant"]
                text_ok = (not vw.get("text_regex")) or bool(re.search(vw["text_regex"], dr.doc_metrics(rows).get("all_text") or "", re.I | re.S))
                if "PASS" in out and text_ok and not any(m.get("status") == "streaming" for m in rows):
                    # the files (and the words, when asked for) are there; let the agent's final note land
                    time.sleep(vw.get("settle_s", 0))
                    break
                time.sleep(20)
            doc = rl.chat_dump(self.project, first_chat)
            rows = [m for m in doc.get("messages", []) if m.get("role") == "assistant"]
            dm_all = dr.doc_metrics(rows)
            if vw.get("text_regex") and not re.search(vw["text_regex"], dm_all.get("all_text") or "", re.I | re.S):
                reasons.append(f"no delivered bubble matches /{vw['text_regex'][:60]}/ within {vw['wait_s']} s")
            if vw.get("text_regex_not") and re.search(vw["text_regex_not"], dm_all.get("all_text") or "", re.I):
                reasons.append(f"a delivered bubble matches forbidden /{vw['text_regex_not'][:50]}/")
            vw_rec = {"verify_out": out[-600:], "waited_s": round(vw["wait_s"] - (deadline - time.time()), 1), "progress_rows": dm_all["progress_rows"], "all_text": (dm_all["all_text"] or "")[:4000]}
            (self.out / f"{tid}.final.doc.json").write_text(json.dumps(doc, indent=1))
            if "PASS" not in out:
                reasons.append(f"background result: {out.strip().splitlines()[-1][:160] if out.strip() else 'no output'}")
            if vw.get("min_progress_rows") and dm_all["progress_rows"] < vw["min_progress_rows"]:
                reasons.append(f"only {dm_all['progress_rows']} progress rows (need {vw['min_progress_rows']})")
            if vw.get("rubric"):
                rb = gr.judge(task["turns"][0]["prompt"], vw["rubric"]["criteria"], dm_all["all_text"], out, out)
                vw_rec["rubric"] = rb
                if rb.get("score") is not None and rb["score"] < vw["rubric"].get("min", 3):
                    reasons.append(f"rubric {rb['score']}/5: {rb['reason']}")
            all_docs.append(doc)
        plc = task.get("peer_lane_checks")
        if plc and plc.get("lane"):
            # the other lane's pinned chat must show the agent-to-agent row (and its reply-to)
            peer_pid = next((pid for ln, pid in self.rs["projects"].items() if f"lane-{ln}" == plc["lane"]), None)
            if peer_pid:
                pin = rl.pinned_chat(peer_pid)
                pdoc = rl.chat_dump(peer_pid, pin) if pin else {"messages": []}
                recent = [m for m in pdoc.get("messages", []) if (m.get("createdAt") or 0) >= int(t_task0 * 1000) - 5000]
                proj_rows = [m for m in recent if m.get("role") == "project" or str(m.get("sender", "")).startswith("project:")]
                texts = " ".join(dr.text_of(m) for m in proj_rows)
                if plc.get("expect_text") and plc["expect_text"] not in texts:
                    reasons.append(f"lane-b's pinned chat shows no agent message carrying {plc['expect_text']!r} ({len(proj_rows)} project rows)")
                if plc.get("verify_peer"):
                    code, out = self.box.run_as_ws(peer_pid, plc["verify_peer"], timeout=60)
                    if "PASS" not in out:
                        reasons.append(f"peer lane check: {out.strip()[-120:]}")
                if plc.get("expect_reply_to"):
                    ids = {m.get("id") for m in proj_rows}
                    answered = [m for m in recent if m.get("role") == "assistant" and m.get("inReplyTo") in ids]
                    if not answered:
                        reasons.append("lane-b's answer does not reply-to the agent message")
        reasons += task_state.get("action_findings") or []
        dc = task.get("doc_checks")
        if dc:
            checks = doc_checks(all_docs, after_ms=task_state.get("renamed_at_ms"))
            if dc.get("max_handoffs") is not None and checks["handoffs"] > dc["max_handoffs"]:
                reasons.append(f"{checks['handoffs']} handoffs (continue-act) > {dc['max_handoffs']}")
            if dc.get("no_duplicate_status") and checks["dup_status"]:
                reasons.append(f"duplicated status lines: {checks['dup_status'][:3]}")
            bg_chats = {t["chat_id"] for t, spec in zip(turns_out, task["turns"]) if spec.get("background")}
            if dc.get("no_empty_result_rows") and checks["empty_result_rows"] and not bg_chats:
                reasons.append(f"{checks['empty_result_rows']} result row(s) with tool cards but no text (reply lost)")
            if dc.get("no_repeated_sentence") and checks["repeated_sentences"]:
                reasons.append(f"a sentence appears twice in one bubble: {checks['repeated_sentences'][:2]}")
            if dc.get("no_truncated_reply") and checks["truncated"]:
                reasons.append(f"a done bubble ends mid-sentence: {checks['truncated'][:2]}")
            if dc.get("require_part_kind"):
                # a structured part (e.g. a `chart` from post_chart) must exist in an assistant row of this task
                want = dc["require_part_kind"]
                # the post can land a few seconds after the turn's last dump: read the chats again now
                fresh = []
                for c in {m.get("chat_id") for m in turns_out if m.get("chat_id")}:
                    try:
                        fresh.append(rl.chat_dump(self.project, c))
                    except Exception:  # noqa: BLE001
                        pass
                found = [p for d in (fresh or all_docs) for m in d.get("messages", []) if m.get("role") == "assistant" for p in m.get("parts", []) if p.get("kind") == want]
                if not found:
                    reasons.append(f"no assistant row carries a `{want}` part (the tool was not used or its post did not land)")
                else:
                    task_state[f"parts_{want}"] = [{k: v for k, v in p.items() if k != "chart"} | ({"series": len((p.get("chart") or {}).get("series") or [])} if want == "chart" else {}) for p in found]
            if dc.get("forbid_part_kind"):
                # e.g. an `approval` card for a send after "draft it, do NOT send anything" (m12)
                bad = dc["forbid_part_kind"]
                hits = [p.get("title") or p.get("id") for d in all_docs for m in d.get("messages", []) if m.get("role") == "assistant" for p in m.get("parts", []) if p.get("kind") == bad]
                if hits:
                    reasons.append(f"an assistant row carries a `{bad}` part the task forbids: {hits[:2]}")
            if dc.get("no_wrong_self_identity"):
                # a bubble from the renamed agent must never introduce itself by the old name
                old_name = task_state.get("old_name") or dc.get("old_name")
                if old_name:
                    rx = r"\b(I['’]m|I am|my name is|call me|this is|you['’]re talking to|speaking)\s+" + re.escape(old_name) + r"\b|\b" + re.escape(old_name) + r" here\b"
                    hits = [t[:80] for t in checks["texts_after_rename"] if re.search(rx, t, re.I)]
                    if hits:
                        reasons.append(f"a bubble after the rename still says it is {old_name!r}: {hits[:2]}")
            if dc.get("agent_replies_carry_reply_to") and checks["unquoted_replies"]:
                reasons.append(f"{len(checks['unquoted_replies'])} agent reply row(s) without replyTo = the asking message (#1521): {checks['unquoted_replies'][:3]}")
            if dc.get("forbid_text_regex"):
                hits = [t[:80] for t in checks["texts"] if re.search(dc["forbid_text_regex"], t, re.I)]
                if hits:
                    reasons.append(f"raw error text shown to the user: {hits[:2]}")
            if dc.get("failure_lines_exactly") is not None:
                n = sum(1 for t in checks["texts"] if re.search(dc.get("failure_regex", "could not|couldn't|failed|unable|did not start|didn't start"), t, re.I))
                if n != dc["failure_lines_exactly"]:
                    reasons.append(f"{n} failure line(s) shown, expected exactly {dc['failure_lines_exactly']}")
        for act in task.get("cleanup_actions") or []:
            try:
                self.driver_action(act, task_state)
            except Exception as e:
                self.say(f"{tid}: cleanup action failed: {e}")
        if not task.get("keep_dir"):
            # the eval's own artefacts must not fill the 2 GiB workspace disk (#1641): the task dir
            # and anything the agent installed into the workspace home for it go after grading
            self.box.run_as_ws(self.project, f"rm -rf {D} 2>/dev/null; rm -rf {proj_dir}/.arbos/home/.rustup {proj_dir}/.arbos/home/.cargo 2>/dev/null; true", timeout=120)
        if task.get("cleanup"):
            self.box.run_as_ws(self.project, task["cleanup"].replace("{D}", D).replace("{P}", proj_dir).replace("{RUN}", tag), env=env, timeout=120)
        if task.get("cleanup_routines"):
            for r in rl.routines(self.project):
                if r.get("id"):
                    rl.delete_routine(self.project, r["id"])
        usage1 = rl.usage_all(self.project)
        rel = rl.diff_usage(usage0, usage1)
        judge_usd = sum(((t.get("rubric") or {}).get("usd") or 0) for t in turns_out) + (((vw_rec or {}).get("rubric") or {}).get("usd") or 0)
        rec = {
            "task": tid,
            "title": task.get("title"),
            "category": task["category"],
            "categories": task.get("categories") or [task["category"]],
            "lane": self.lane,
            "regression": task.get("regression"),
            "success": not reasons,
            "reasons": reasons,
            "ttfr_s": turns_out[0]["ttfr_s"] if turns_out else None,
            "first_seen_s": min([x for x in (turns_out[0]["ttfr_s"], turns_out[0]["first_status_s"]) if x is not None] or [None]) if turns_out else None,
            # the first turn's route decides the cost shape: `new` = a Sonnet helper (≈ 5× an `act` turn on Haiku)
            "route_t1": next((r for t in turns_out for r in (t.get("routes") or []) if r), None),
            "ttfr_all_s": [t["ttfr_s"] for t in turns_out],
            "total_s": round(time.time() - t_task0, 1),
            "tool_errors": sum(t["tool_errors"] or 0 for t in turns_out),
            "stalls": sum(t["stalls"] for t in turns_out) + sum(1 for t in turns_out if t["timed_out"]),
            "cost_usd": rel.get("usd", 0.0),
            "judge_usd": round(judge_usd, 5),
            "relay": rel,
            "rubric_min": min([t["rubric"]["score"] for t in turns_out if t.get("rubric") and t["rubric"].get("score") is not None] or [None]) if any(t.get("rubric") for t in turns_out) else None,
            "turns": turns_out,
            "background": vw_rec,
            "dir": D,
        }
        self.say(f"{tid}: {'PASS' if rec['success'] else 'FAIL'} {reasons[:3]} | ${rec['cost_usd']:.4f} | {rec['total_s']}s")
        return rec


def doc_checks(docs, after_ms=None):
    """Whole-transcript checks over every chat doc a task touched (rows are deduplicated by id)."""
    handoffs = 0
    dups, repeated, texts, truncated, texts_after, unquoted = [], [], [], [], [], []
    empty_result_rows = 0
    seen_rows = set()
    for doc in docs:
        for m in doc.get("messages", []):
            if m.get("role") != "assistant" or m.get("id") in seen_rows:
                continue
            seen_rows.add(m.get("id"))
            s = json.dumps(m)
            if m.get("escalatedFromAct") or m.get("escalated_from_act") or "continue-act" in s:
                handoffs += 1
            seen = set()
            for p in m.get("parts", []):
                if p.get("kind") == "status" and not p.get("hidden"):
                    key = (p.get("code"), (p.get("message") or "").strip())
                    if key in seen and key[1]:
                        dups.append(key[1][:60])
                    seen.add(key)
            text = dr.text_of(m)
            tools = [p for p in m.get("parts", []) if p.get("kind") == "tool"]
            # #1521: a finished reply with words that answers a user row carries the platform
            # reply (`replyTo` = that row) so the bubble quotes the question (groups: always).
            if text and m.get("status") == "done" and m.get("inReplyTo") and m.get("replyTo") != m.get("inReplyTo") and m.get("turnRole") != "ack":
                unquoted.append(m.get("id"))
            if text:
                texts.append(text)
                if after_ms and (m.get("createdAt") or 0) >= after_ms:
                    texts_after.append(text)
                # a finished bubble that stops on a comma / dash / mid-word (no closing punctuation,
                # bracket, quote or code tick) was cut off (#1411)
                # a bubble that ends on a markdown table row (`| … |`) or a code fence is complete
                if m.get("status") == "done" and len(text) > 30 and not re.search(r"[.!?…:)\]\"'`*_\d|]\s*$|```\s*$|\n\s*[-•*\d].*$|^[\w ,\-]+$|^(\w[\w\- ]*,\s*)+\w[\w\- ]*$", text):
                    truncated.append(text[-60:])
                sentences = [x.strip() for x in re.split(r"(?<=[.!?])\s+", text) if len(x.strip()) > 25]
                for x in sentences:
                    if sentences.count(x) > 1 and x[:60] not in repeated:
                        repeated.append(x[:60])
            elif tools and m.get("turnRole") != "ack" and m.get("status") == "done":
                empty_result_rows += 1
    return {"handoffs": handoffs, "dup_status": dups, "empty_result_rows": empty_result_rows, "repeated_sentences": repeated, "texts": texts, "truncated": truncated, "texts_after_rename": texts_after, "unquoted_replies": unquoted}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--tasks", default="")
    ap.add_argument("--subset", default="", choices=["", *SUBSETS])
    ap.add_argument("--lanes", default="")
    ap.add_argument("--budget-usd", type=float, default=float(os.environ.get("LT_BUDGET_USD", "8")))
    ap.add_argument("--run-id", default=time.strftime("%Y%m%dT%H%M"))
    ap.add_argument("--out", default="")
    ap.add_argument("--dry-run", action="store_true", help="list the tasks per lane and exit")
    ap.add_argument("--min-headroom-usd", type=float, default=float(os.environ.get("LT_MIN_HEADROOM_USD", "1.0")), help="refuse to start when the relay cap has less headroom than this (#1736 rules)")
    ap.add_argument("--repeat", default="", help="run some tasks several times, e.g. 'v2-vault-env-secret:5' (comma list); copies get ids <id>#2…#N for flake counts")
    ap.add_argument("--skip-parity", action="store_true", help="leave the Grok-parity prompts out (the duplex owner runs their own scorecards; they are ~55 %% of a full run's cost)")
    ap.add_argument("--allow-stale-seed", action="store_true", help="run even when the box's pi-ext seed does not match its image commit (#1702)")
    ap.add_argument("--refresh-stale-seed", action="store_true", help="when the seed is stale, re-copy /etc/arbos from the image into the box's persisted /etc and restart it (test box only)")
    args = ap.parse_args()
    only = set(args.tasks.split(",")) if args.tasks else None
    if args.subset:
        only = (only or set()) | set(SUBSETS[args.subset])
    lanes = set(args.lanes.split(",")) if args.lanes else None
    tasks = load_tasks(only, lanes)
    if args.skip_parity:
        tasks = [t for t in tasks if not t.get("parity")]
    if args.repeat:
        import copy
        reps = {k: int(v) for k, v in (x.split(":") for x in args.repeat.split(",") if ":" in x)}
        extra = []
        for t in tasks:
            for n in range(2, reps.get(t["id"], 1) + 1):
                c = copy.deepcopy(t)
                c["id"] = f"{t['id']}-r{n}"
                c["repeat_of"] = t["id"]
                extra.append(c)
        tasks += extra
        tasks.sort(key=lambda t: (t["lane"], t.get("same_chat_as") or t["id"], 0 if not t.get("same_chat_as") else 1, t["id"]))
    if args.dry_run:
        for ln in LANES:
            print(ln, [t["id"] for t in tasks if t["lane"] == ln])
        return
    rl.ensure_test_user(TEST_HANDLE, "Long-task eval")
    # every run on its own signed-in device: a refresh token consumed by another process (a waiter,
    # a check, a crashed run) cannot take the run down mid-way
    print(f"preflight: device {rl.fresh_device(f'run {args.run_id}')} minted for this run", flush=True)
    m = rl.machine()
    if m.get("state") != "awake":
        print("machine not awake; starting", flush=True)
        rl.ctl("machine", "start")
        for _ in range(30):
            time.sleep(10)
            if rl.machine().get("state") == "awake":
                break
    box = bx.Box()
    code, out = box.root("df -m /work | tail -1 | awk '{print $4}'")
    try:
        free_mib = int(out.strip().splitlines()[-1])
    except Exception:
        free_mib = -1
    print(f"preflight: /work free {free_mib} MiB", flush=True)
    if 0 <= free_mib < 300:
        raise SystemExit(f"preflight: only {free_mib} MiB free on /work — clean the box first (#1641)")
    # #1702: the box must run the pi extension its image was built from; a stale seed measures the wrong code
    seed = box.pi_seed(HERE.parent.parent)
    seed_line = lambda s: f"pi-ext seed {'ok' if s['ok'] else 'MISMATCH' if s['ok'] is False else 'unverified'} — {s['reason']} (image {s['image']}, source_sha {str(s.get('source_sha'))[:12]}, tag commit {str(s.get('tag_commit'))[:12]})"  # noqa: E731
    print(f"preflight: {seed_line(seed)}", flush=True)
    print(f"preflight: relay binary {box.relay_build()}", flush=True)
    if seed["ok"] is False and args.refresh_stale_seed:
        # the persisted /etc of a host-sandbox box keeps the seed of the image that created it (fleet.rs
        # seed_etc runs once); re-copy /etc/arbos from the current image and restart so agentd re-seeds
        code, out = box.refresh_etc_arbos()
        print(f"preflight: refreshed /etc/arbos from the image (rc={code}): {out[-200:]}", flush=True)
        rl.ctl("machine", "stop", "--force", timeout=180)
        time.sleep(5)
        rl.ctl("machine", "start", timeout=180)
        for _ in range(30):
            if rl.machine().get("state") == "awake":
                break
            time.sleep(10)
        rl.relay("PUT", "/me/secrets/settings", {"paused": False})
        time.sleep(20)
        box = bx.Box()
        seed = box.pi_seed(HERE.parent.parent)
        seed["refreshed"] = True
        print(f"preflight: after refresh, {seed_line(seed)}", flush=True)
    if seed["ok"] is not True and not args.allow_stale_seed:
        raise SystemExit(f"preflight: blocked — {seed['reason']}; not a result (pass --allow-stale-seed to run anyway)")
    spent, cap = rl.spent_today()
    if cap and cap - spent < args.budget_usd:
        print(f"WARNING: daily cap headroom ${cap - spent:.2f} < budget ${args.budget_usd}; the relay will stop the agent first", flush=True)
    if cap and cap - spent < args.min_headroom_usd:
        raise SystemExit(f"preflight: blocked — lt-eval spent ${spent:.2f} of ${cap:.2f} today, headroom ${cap - spent:.2f} < ${args.min_headroom_usd} (--min-headroom-usd); not a result")
    out_dir = Path(args.out or (HERE / "results" / args.run_id))
    out_dir.mkdir(parents=True, exist_ok=True)
    log = (out_dir / "run.log").open("a")
    run_state = {"run_id": args.run_id, "budget": args.budget_usd, "spent_at_start": spent, "lock": threading.Lock()}
    projects = {ln: rl.ensure_project(f"lane-{ln}") for ln in LANES}
    run_state["projects"] = projects
    late = [t for t in tasks if t["lane"] in LATE_LANES]
    tasks = [t for t in tasks if t["lane"] not in LATE_LANES]
    for t in late:
        t["lane"] = "a"  # late tasks run on lane a's project, after the parallel lanes
    projects = {ln: pid for ln, pid in projects.items() if any(t["lane"] == ln for t in tasks + late)}
    meta = {
        "run_id": args.run_id,
        "started_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "image": box.image,
        "agentd": box.agentd_sha(),
        "pi_seed": seed,
        "relay_build": box.relay_build(),
        "provider": box.provider,
        "user": box.user,
        "projects": projects,
        "tasks": len(tasks),
        # "full" (the scheduled run), a named subset, or "tasks" for a hand-picked rerun; the report compares like with like
        "subset": args.subset or ("tasks" if args.tasks else "full"),
        "budget_usd": args.budget_usd,
        "spent_today_at_start": spent,
        "judge_model": gr.JUDGE_MODEL,
    }
    print(json.dumps(meta, indent=1), flush=True)
    lanes_t = [Lane(ln, projects[ln], [t for t in tasks if t["lane"] == ln], box, run_state, out_dir, log) for ln in projects]
    for l in lanes_t:
        l.start()
    for l in lanes_t:
        l.join()
    results = [r for l in lanes_t for r in l.results]
    if late:
        late_lane = Lane("a", projects["a"], late, box, run_state, out_dir, log)
        late_lane.start()
        late_lane.join()
        results += late_lane.results
    # Nothing of the run may keep spending after it: delete every routine on the lane projects. s1 creates
    # an every-minute ticker that s3 deletes two tasks later — in run 7 s3 was skipped on budget and the
    # ticker fired a model turn a minute for 14 h, which emptied the day's cap (2026-10-26).
    swept = 0
    for ln, pid in run_state["projects"].items():
        try:
            for r in rl.routines(pid):
                if r.get("id"):
                    rl.delete_routine(pid, r["id"])
                    swept += 1
        except Exception as e:  # noqa: BLE001
            print(f"routine sweep on lane {ln} failed: {e}", flush=True)
    if swept:
        print(f"end-of-run sweep: deleted {swept} leftover routine(s)", flush=True)
    spent_end, _ = rl.spent_today()
    meta["ended_at"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    meta["relay_build_at_end"] = box.relay_build()
    if meta.get("relay_build") and meta["relay_build_at_end"] != meta["relay_build"]:
        print(f"NOTE: the relay binary changed during the run: {meta['relay_build']} -> {meta['relay_build_at_end']}; every task reads rows through a client path (arbosctl), so verdicts around the change are suspect", flush=True)
    meta["spent_today_at_end"] = spent_end
    # the relay's "spent today" resets at 00:00 UTC; a run across midnight reads negative, so fall back to the per-task sum
    run_cost = spent_end - spent
    if run_cost < 0:
        run_cost = sum(r.get("cost_usd") or 0 for r in results)
    meta["run_cost_usd"] = round(run_cost, 4)
    meta["judge_cost_usd"] = round(sum(r.get("judge_usd") or 0 for r in results), 4)
    board = summarize(meta, results)
    sb_dir = HERE / "scoreboard"
    sb_dir.mkdir(exist_ok=True)
    (sb_dir / f"{args.run_id}.json").write_text(json.dumps(board, indent=1))
    (out_dir / "scoreboard.json").write_text(json.dumps(board, indent=1))
    print(json.dumps({k: board[k] for k in ("summary",)}, indent=1), flush=True)
    print(f"scoreboard: {sb_dir / (args.run_id + '.json')}")


def pct(n, d):
    return round(100.0 * n / d, 1) if d else None


def median(xs):
    xs = sorted(x for x in xs if x is not None)
    if not xs:
        return None
    n = len(xs)
    return xs[n // 2] if n % 2 else round((xs[n // 2 - 1] + xs[n // 2]) / 2, 2)


def summarize(meta, results):
    done = [r for r in results if not r.get("skipped")]
    passed = [r for r in done if r["success"]]
    cats = {}
    for r in results:
        for c in r.get("categories") or [r["category"]]:
            cats.setdefault(c, {"tasks": 0, "passed": 0})
            cats[c]["tasks"] += 1
            cats[c]["passed"] += 1 if r["success"] else 0
    for c in cats.values():
        c["pass_pct"] = pct(c["passed"], c["tasks"])
    summary = {
        "tasks": len(results),
        "run": len(done),
        "passed": len(passed),
        "failed": len(done) - len(passed),
        "skipped": len(results) - len(done),
        "pass_pct": pct(len(passed), len(done)),
        "ttfr_p50_s": median([r.get("ttfr_s") for r in done]),
        "ttfr_all_p50_s": median([t for r in done for t in (r.get("ttfr_all_s") or [])]),
        "total_p50_s": median([r.get("total_s") for r in done]),
        "tool_errors": sum(r.get("tool_errors") or 0 for r in done),
        "stalls": sum(r.get("stalls") or 0 for r in done),
        "cost_usd": round(sum(r.get("cost_usd") or 0 for r in done), 4),
        "run_cost_usd": meta.get("run_cost_usd"),
        "judge_cost_usd": meta.get("judge_cost_usd"),
        "categories": cats,
        "parity": py.summarize(results),
        "convergence": cv.summarize(results),
    }
    return {"meta": meta, "summary": summary, "results": results}


if __name__ == "__main__":
    main()

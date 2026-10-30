"""Grok-parity tasks (docs/duplex-agent-design.md §4): how the agent *talks*, not only what it gets right.

A parity task (`"parity": true`) is a scripted conversation: turns are sent at offsets (`at_s`) without
waiting for the previous turn, a `stop` command can be fired, and the whole chat is watched to the end.
Per assistant message the harness records order, timing, verbatim text, formatting features, threading
(`replyTo`), and whether a background worker's row was visible; the project's status line is sampled every
`SAMPLE_S`. Deterministic rules decide the traits (speed, ack-first, cadence, formatting, no-handle,
worker-hidden, threading, busy-reply, stop, status-line, offer); an LLM judge compares the messages with the
Grok Bot baseline transcript (`baselines/grok/<task>.md`) when one exists.

Trait results land in the task record under `parity`, and `run.summarize` folds them into the scoreboard
(`summary.parity`): per-trait pass counts and the judge means per trait.
"""

import json
import re
import threading
import time
from pathlib import Path

from . import driver as dr
from . import grade as gr
from . import relay as rl

HERE = Path(__file__).resolve().parent.parent
SAMPLE_S = 0.5
DOC_POLL_S = 1.0
GENERIC_ACTIVITY = re.compile(r"^(working|thinking|checking on it|working on it)[.…]*$|is working$", re.I)
HANDLE_RX = re.compile(r"(?<![\w/.`])@[A-Za-z][\w-]{1,}|\b(lt-eval|unarbos)\b", re.I)
URL_RX = re.compile(r"https?://[^\s)\]>\"'`,]+|\[[^\]]+\]\([^)]+\)")
OFFER_RX = re.compile(r"\?\s*$|\b(want me to|shall i|should i|would you like|happy to|say the word|let me know if you want)\b", re.I | re.M)
JUDGE_TRAITS = ["shape", "formatting", "voice", "sources", "next_step", "coverage"]
RULE_TRAITS = ["speed", "ack_first", "cadence", "formatting", "no_handle", "worker_hidden", "threading", "busy_reply", "stop", "status_line", "offer", "silence"]


# ---- message features ----------------------------------------------------------------------------


def has_table(text):
    rows = [l for l in text.splitlines() if l.strip().startswith("|") and l.strip().endswith("|")]
    return len(rows) >= 3 and any(re.match(r"^\|?\s*:?-{2,}", l.strip()) or re.search(r"\|\s*:?-{2,}", l) for l in rows)


def has_bullets(text):
    return sum(1 for l in text.splitlines() if re.match(r"^\s*([-*•]|\d+[.)])\s+\S", l)) >= 2


def sentences(text):
    return [s for s in re.split(r"(?<=[.!?])\s+", text.strip()) if s.strip()]


def is_worker_row(m):
    """A background worker's row a client would draw ("N messages with <name>"): the spawn/wake row
    of a subagent (turn kind spawn|new|wake, or a subagent row with no words) that is not flagged
    `worker: true` (#1673). The result bubble the main agent writes for a finished subagent also
    carries `subagentId`, but it is the answer, not a worker row."""
    if not (m.get("subagentId") or m.get("subagentName") or m.get("handoffMessages")):
        return False
    if m.get("worker"):
        return False
    kind = ((m.get("turn") or {}).get("kind") if isinstance(m.get("turn"), dict) else None)
    return kind in ("spawn", "new", "wake") or not dr.text_of(m)


def message_record(m, index, send_at_ms, first_seen_s):
    text = dr.text_of(m)
    created = m.get("createdAt") or 0
    return {
        "index": index,
        "id": m.get("id"),
        "created_at": created,
        "completed_at": m.get("completedAt"),
        "t_doc_s": round((created - send_at_ms) / 1000.0, 2) if created and send_at_ms else None,
        "t_seen_s": first_seen_s,
        "t_done_s": round(((m.get("completedAt") or 0) - send_at_ms) / 1000.0, 2) if m.get("completedAt") and send_at_ms else None,
        "status": m.get("status"),
        "turn_role": m.get("turnRole"),
        "decision": m.get("decision"),
        "in_reply_to": m.get("inReplyTo"),
        "reply_to": m.get("replyTo"),
        "text": text,
        "word_count": len(text.split()),
        "sentences": len(sentences(text)),
        "has_table": has_table(text),
        "has_bullets": has_bullets(text),
        "link_count": len(URL_RX.findall(text)),
        "header_count": sum(1 for l in text.splitlines() if re.match(r"^\s*#{1,4}\s|\*\*[^*]+\*\*\s*$", l.strip())),
        "addresses_handle": bool(HANDLE_RX.search(text)),
        "is_worker_row": is_worker_row(m),
        "worker_flag": bool(m.get("worker")),
        "has_error": any(p.get("kind") == "error" for p in m.get("parts", [])),
        "has_question": any(p.get("kind") == "question" for p in m.get("parts", [])),
        "has_approval": any(p.get("kind") == "approval" for p in m.get("parts", [])),
    }


# ---- watching the chat(s) --------------------------------------------------------------------------


class ChatWatch(threading.Thread):
    """Polls one chat's doc and remembers when each assistant row was first seen with words."""

    def __init__(self, project, chat_id, t0):
        super().__init__(daemon=True)
        self.project, self.chat_id, self.t0 = project, chat_id, t0
        self.doc = {"messages": []}
        self.first_seen = {}  # row id -> seconds since t0 when it first had text (or a done status)
        self.first_row_seen = {}  # row id -> seconds when the row first existed
        self.last_change = time.time()
        self.fp = None
        self.stop_flag = threading.Event()
        self.lock = threading.Lock()

    def run(self):
        while not self.stop_flag.is_set():
            try:
                doc = rl.chat_dump(self.project, self.chat_id)
            except Exception:
                doc = None
            now = time.time()
            if doc and doc.get("messages") is not None:
                with self.lock:
                    self.doc = doc
                    for m in doc.get("messages", []):
                        if m.get("role") != "assistant":
                            continue
                        rid = m.get("id")
                        self.first_row_seen.setdefault(rid, round(now - self.t0, 2))
                        if rid not in self.first_seen and (dr.text_of(m) or m.get("status") in ("done", "error", "failed", "cancelled", "stopped")):
                            self.first_seen[rid] = round(now - self.t0, 2)
                    fp = dr.doc_fingerprint([m for m in doc.get("messages", []) if m.get("role") == "assistant"])
                    if fp != self.fp:
                        self.fp, self.last_change = fp, now
            self.stop_flag.wait(DOC_POLL_S)

    def snapshot(self):
        with self.lock:
            return json.loads(json.dumps(self.doc)), dict(self.first_seen), dict(self.first_row_seen), self.last_change

    def streaming(self):
        with self.lock:
            return any(m.get("status") == "streaming" for m in self.doc.get("messages", []) if m.get("role") == "assistant")


class ActivitySampler(threading.Thread):
    def __init__(self, project, t0):
        super().__init__(daemon=True)
        self.project, self.t0 = project, t0
        self.samples = []  # (t, text or None)
        self.stop_flag = threading.Event()

    def run(self):
        last = object()
        while not self.stop_flag.is_set():
            try:
                a = rl.project_activity(self.project)
            except Exception:
                a = None
            if a != last:
                self.samples.append((round(time.time() - self.t0, 2), a))
                last = a
            self.stop_flag.wait(SAMPLE_S)


def find_user_msg(doc, prompt, after_ms):
    users = [m for m in doc.get("messages", []) if m.get("role") == "user" and dr.text_of(m) == prompt and (m.get("createdAt") or 0) >= after_ms - 5000]
    return users[-1] if users else None


def wait_settled(watches, settle_s, deadline, since_ms=None, in_chat=None):
    """Wait until the agent has answered (at least one assistant row created after `since_ms` in
    `in_chat`, when given), no chat has a streaming row, and nothing changed for `settle_s`; or the
    deadline passes. Without the row condition a slow first row (an act turn's setup) would read as
    "settled" before the agent said anything."""
    while time.time() < deadline:
        if since_ms is not None and in_chat is not None:
            doc, _, _, _ = in_chat.snapshot()
            answered = any(m.get("role") == "assistant" and (m.get("createdAt") or 0) >= since_ms - 2000 for m in doc.get("messages", []))
            if not answered:
                time.sleep(1)
                continue
        if not any(w.streaming() for w in watches) and all(time.time() - w.last_change >= settle_s for w in watches):
            return True
        time.sleep(1)
    return False


# ---- the task ----------------------------------------------------------------------------------------


def run_task(lane, task):
    """Drive one parity task; returns the task record (same shape as run.Lane.run_task plus `parity`)."""
    tid = task["id"]
    tag = lane.rs["run_id"]
    project = lane.project
    proj_dir = f"/work/{project}"
    D = f"{proj_dir}/lt-{tid}-{tag}"
    env = {"D": D, "P": proj_dir}
    lane.say(f"{tid}: parity start ({D})")
    t_task0 = time.time()
    usage0 = rl.usage_all(project)
    gen_truth = {}
    lane.box.run_as_ws(project, f"mkdir -p {D}", env=env, timeout=60)
    for turn in task["turns"]:
        if turn.get("seed"):
            code, out = lane.box.run_as_ws(project, turn["seed"].replace("{D}", D).replace("{P}", proj_dir), env=env, timeout=180)
            if code not in (0, None):
                lane.say(f"{tid}: seed rc={code}: {out[-200:]}")
        if turn.get("generate"):
            from . import fixtures as fx
            files, gen_truth = fx.GENERATORS[turn["generate"]]()
            ok, msg = lane.box.put_files(project, D, files)
            if not ok:
                lane.say(f"{tid}: fixture upload failed: {msg}")

    main_chat = rl.chat_open(project, f"{tid} {tag}")
    t0 = time.time()
    deadline = t0 + task.get("timeout_s", 300)
    sampler = ActivitySampler(project, t0)
    sampler.start()
    watches = {main_chat: ChatWatch(project, main_chat, t0)}
    watches[main_chat].start()
    time.sleep(1.5)

    sent = []  # per turn: dict(turn index, kind, chat, prompt, send_at (s since t0), send_at_ms, user_msg_id)
    procs = []
    prev_send = t0
    for i, turn in enumerate(task["turns"], 1):
        kind = turn.get("kind", "prompt")
        prompt = (turn.get("prompt") or "").replace("{D}", D).replace("{P}", proj_dir)
        if turn.get("at_s") is not None:
            target = prev_send + turn["at_s"]
            while time.time() < target:
                time.sleep(0.05)
        elif i > 1:
            # sequential turn: wait for the previous one to be answered and settle first
            prev = next((r for r in reversed(sent) if r["kind"] == "prompt"), None)
            wait_settled(list(watches.values()), task.get("settle_s", 12), min(deadline, time.time() + turn.get("timeout_s", task.get("timeout_s", 300))),
                         since_ms=prev["send_at_ms"] if prev else None, in_chat=watches.get(prev["chat_id"]) if prev else None)
        chat = main_chat
        if turn.get("new_chat"):
            chat = rl.chat_open(project, f"{tid} {tag} t{i}")
            watches[chat] = ChatWatch(project, chat, t0)
            watches[chat].start()
            time.sleep(1.0)
        rec = {"turn": i, "kind": kind, "chat_id": chat, "prompt": prompt, "send_at_s": None, "send_at_ms": None, "user_msg_id": None}
        if kind == "prompt":
            send_at = time.time()
            # non-blocking: back-to-back turns (`at_s: 1`) must leave within the second
            procs.append(rl.ctl_popen("chat", "send", project, chat, prompt, "--timeout", "2"))
            rec["send_at_s"] = round(send_at - t0, 2)
            rec["send_at_ms"] = int(send_at * 1000)
            prev_send = send_at
            lane.say(f"{tid} t{i}: sent at +{rec['send_at_s']}s → {prompt[:60]!r}")
        elif kind == "stop":
            send_at = time.time()
            rl.ctl("chat", "stop", project, chat, timeout=60)
            rec["send_at_s"] = round(send_at - t0, 2)
            rec["send_at_ms"] = int(send_at * 1000)
            prev_send = send_at
            lane.say(f"{tid} t{i}: stop at +{rec['send_at_s']}s")
        elif kind == "wait":
            rec["send_at_s"] = round(time.time() - t0, 2)
            rec["send_at_ms"] = int(time.time() * 1000)
            prev_send = time.time()
        sent.append(rec)
        if turn.get("wait_done"):
            wait_settled(list(watches.values()), task.get("settle_s", 12), min(deadline, time.time() + turn.get("timeout_s", task.get("timeout_s", 300))),
                         since_ms=rec["send_at_ms"] if kind == "prompt" else None, in_chat=watches.get(chat))

    # let the conversation finish: the last prompt answered, then quiet
    last_prompt = next((r for r in reversed(sent) if r["kind"] == "prompt"), None)
    settled = wait_settled(list(watches.values()), task.get("settle_s", 12), deadline,
                           since_ms=last_prompt["send_at_ms"] if last_prompt else None, in_chat=watches.get(last_prompt["chat_id"]) if last_prompt else None)
    time.sleep(2)
    sampler.stop_flag.set()
    for pr in procs:
        try:
            pr.kill()
        except Exception:  # noqa: BLE001
            pass
    for w in watches.values():
        w.stop_flag.set()
    for w in watches.values():
        w.join(timeout=5)
    if not settled:
        for chat in watches:
            rl.ctl("chat", "stop", project, chat, timeout=60)

    # ---- evidence ------------------------------------------------------------------------------
    docs = {c: w.snapshot() for c, w in watches.items()}
    for r in sent:
        if r["kind"] == "prompt" and not r.get("user_msg_id"):
            doc = docs[r["chat_id"]][0]
            u = find_user_msg(doc, r["prompt"], r["send_at_ms"])
            if u:
                r["user_msg_id"] = u["id"]
    user_ids = {r["user_msg_id"]: r for r in sent if r.get("user_msg_id")}
    messages = []
    for chat, (doc, first_seen, first_row, _) in docs.items():
        rows = [m for m in doc.get("messages", []) if m.get("role") == "assistant"]
        rows.sort(key=lambda m: (m.get("createdAt") or 0))
        for idx, m in enumerate(rows, 1):
            owner = user_ids.get(m.get("inReplyTo"))
            if owner is None:
                # not threaded to an ask: attribute to the latest prompt sent before it in this chat
                cands = [r for r in sent if r["chat_id"] == chat and r["kind"] == "prompt" and r["send_at_ms"] and r["send_at_ms"] <= (m.get("createdAt") or 0) + 1500]
                owner = cands[-1] if cands else None
            send_ms = owner["send_at_ms"] if owner else None
            seen = first_seen.get(m.get("id"))
            seen_rel = round(seen - owner["send_at_s"], 2) if (seen is not None and owner and owner["send_at_s"] is not None) else None
            r = message_record(m, idx, send_ms, seen_rel)
            r["chat_id"] = chat
            r["turn"] = owner["turn"] if owner else None
            r["threaded"] = owner is not None and m.get("inReplyTo") == owner.get("user_msg_id")
            r["visible"] = bool(r["text"]) or r["has_question"] or r["has_approval"] or r["has_error"] or r["is_worker_row"]
            messages.append(r)
    messages.sort(key=lambda r: (r["created_at"] or 0))
    (lane.out / f"{tid}.messages.json").write_text(json.dumps({"sent": sent, "messages": messages}, indent=1))
    (lane.out / f"{tid}.status_line.json").write_text(json.dumps(sampler.samples, indent=1))
    for chat, (doc, _, _, _) in docs.items():
        (lane.out / f"{tid}.{chat[-6:]}.doc.json").write_text(json.dumps(doc, indent=1))

    # ---- rules per turn --------------------------------------------------------------------------
    traits = {t: [] for t in RULE_TRAITS}
    reasons = []
    turns_out = []
    judge_scores = {t: [] for t in JUDGE_TRAITS}
    judge_usd = 0.0
    baseline_text = ""
    if task.get("baseline"):
        bp = HERE / task["baseline"]
        if bp.exists():
            baseline_text = bp.read_text()
    for turn, rec in zip(task["turns"], sent):
        i = rec["turn"]
        shape = turn.get("shape") or {}
        kind = rec["kind"]
        mine = [m for m in messages if m["turn"] == i and m["visible"]]
        bubbles = [m for m in mine if m["text"] and not m["is_worker_row"]]
        worker_rows = [m for m in mine if m["is_worker_row"]]
        tr = {}
        treasons = []

        def mark(name, ok, detail):
            tr[name] = {"pass": ok, "detail": detail}
            if ok is not None:
                traits[name].append(ok)
            if ok is False:
                treasons.append(f"{name}: {detail}")

        if kind == "prompt":
            first_s = min([m["t_seen_s"] for m in bubbles if m["t_seen_s"] is not None] or [None])
            lim = shape.get("first_bubble_max_s")
            if lim is not None:
                mark("speed", first_s is not None and first_s <= lim, f"first bubble {first_s} s (limit {lim} s)")
            n = len(bubbles)
            lo, hi = shape.get("messages_min", 1), shape.get("messages_max", 99)
            if shape.get("messages_min") is not None or shape.get("messages_max") is not None:
                mark("cadence", lo <= n <= hi, f"{n} bubbles (want {lo}–{hi})")
            if shape.get("ack_first"):
                ack_ok = bool(bubbles) and bubbles[0]["sentences"] <= 2 and bubbles[0]["word_count"] <= 45 and first_s is not None and first_s <= (shape.get("first_bubble_max_s") or 5)
                mark("ack_first", ack_ok, f"first bubble {bubbles[0]['word_count'] if bubbles else 0} words at {first_s} s" if bubbles else "no bubble")
            fmt_ok, fmt_detail = True, []
            if shape.get("table"):
                t_ok = any(m["has_table"] for m in bubbles)
                fmt_ok &= t_ok
                fmt_detail.append("table" if t_ok else "no table")
            if shape.get("bullets"):
                b_ok = any(m["has_bullets"] for m in bubbles)
                fmt_ok &= b_ok
                fmt_detail.append("bullets" if b_ok else "no bullets")
            if shape.get("links_min"):
                links = sum(m["link_count"] for m in bubbles)
                l_ok = links >= shape["links_min"]
                fmt_ok &= l_ok
                fmt_detail.append(f"{links} links (need {shape['links_min']})")
            if shape.get("table") or shape.get("bullets") or shape.get("links_min"):
                mark("formatting", fmt_ok, ", ".join(fmt_detail))
            if shape.get("no_handle", True):
                bad = [m["text"][:60] for m in bubbles if m["addresses_handle"]]
                mark("no_handle", not bad, f"handle in {bad[:2]}" if bad else "none")
            if shape.get("worker_rows") is not None:
                mark("worker_hidden", len(worker_rows) <= shape["worker_rows"], f"{len(worker_rows)} visible worker row(s)")
            if shape.get("reply_to_ask"):
                results = [m for m in bubbles if m["turn_role"] != "ack"]
                unthreaded = [m for m in results if not (m["reply_to"] == rec["user_msg_id"] or m["in_reply_to"] == rec["user_msg_id"])]
                mark("threading", not unthreaded, f"{len(unthreaded)} of {len(results)} result bubbles not under the ask")
            if shape.get("offer"):
                last = bubbles[-1]["text"] if bubbles else ""
                mark("offer", bool(OFFER_RX.search(last.strip().splitlines()[-1] if last.strip() else "")), "last line offers a next step" if bubbles else "no bubble")
            if shape.get("steer") or (turn.get("at_s") is not None and i > 1 and not turn.get("new_chat")) or turn.get("new_chat"):
                # a message while the agent is busy with an earlier ask
                lim2 = shape.get("first_bubble_max_s") or 5
                mark("busy_reply", first_s is not None and first_s <= lim2, f"answered after {first_s} s while busy (limit {lim2} s)")
            if shape.get("ack_first") and sampler.samples:
                start = rec["send_at_s"]
                end = max([m["t_seen_s"] + start for m in bubbles if m["t_seen_s"] is not None] or [start + 30])
                live = [(t, a) for t, a in sampler.samples if a and not GENERIC_ACTIVITY.search(a.strip()) and start <= t <= end + 5]
                first_live = live[0][0] - start if live else None
                lim3 = shape.get("status_line_max_s", 2) + (first_s or 0)
                mark("status_line", bool(live) and first_live <= lim3 + 1.5, f"first live status {first_live} s after send: {live[0][1][:50]!r}" if live else "no live status text during the task")
            if shape.get("question"):
                asked = any(m["has_question"] for m in mine) or (bubbles and bubbles[-1]["text"].rstrip().endswith("?"))
                if not asked and turn.get("expect_question"):
                    treasons.append("no question asked")
            # content checks shared with the rest of the suite
            reply = "\n\n".join(m["text"] for m in bubbles if m["turn_role"] != "ack") or "\n\n".join(m["text"] for m in bubbles)
            spec = {k: v for k, v in turn.items() if k in ("verify", "show", "truth", "reply_regex", "reply_regex_not", "routine", "expect_question")}
            spec["prompt"] = rec["prompt"]
            for k in ("verify", "show"):
                if spec.get(k):
                    spec[k] = spec[k].replace("{D}", D).replace("{P}", proj_dir)
            ctx = {"reply": reply, "all_text": "\n\n".join(m["text"] for m in bubbles), "dm": {}, "res": {}, "gen_truth": gen_truth}
            g = gr.grade_turn(spec, ctx, lane.box, project, env) if spec.keys() - {"prompt"} else {"ok": True, "reasons": [], "verify_out": "", "truth": {}, "rubric": None}
            treasons += g["reasons"]
            # the judge, against the baseline when there is one
            rb = None
            if turn.get("judge") and bubbles:
                rb = judge_parity(rec["prompt"], bubbles, baseline_text, turn["judge"]["criteria"])
                judge_usd += rb.get("usd") or 0
                for t in JUDGE_TRAITS:
                    if rb.get(t) is not None:
                        judge_scores[t].append(rb[t])
                low = [t for t in JUDGE_TRAITS if rb.get(t) is not None and rb[t] < turn["judge"].get("min", 3)]
                if low:
                    treasons.append(f"judge below 3 on {', '.join(low)}: {rb.get('reason', '')[:140]}")
            turns_out.append({"turn": i, "kind": kind, "chat_id": rec["chat_id"], "prompt": rec["prompt"][:300], "ok": not treasons, "reasons": treasons,
                              "ttfr_s": first_s, "bubbles": n, "traits": tr, "judge": rb, "truth": g.get("truth"), "verify_out": g.get("verify_out"),
                              "reply": reply[:1500], "excerpt": excerpt(rec["prompt"], bubbles)})
        elif kind == "stop":
            t_stop = rec["send_at_s"]
            lim = shape.get("stop_within_s", 3)
            quiet = [t for t, a in sampler.samples if t >= t_stop and not a]
            cleared = (quiet[0] - t_stop) if quiet else None
            busy_before = any(a for t, a in sampler.samples if t_stop - 10 <= t < t_stop)
            if busy_before or any(m["status"] in ("streaming",) for m in messages):
                mark("stop", cleared is not None and cleared <= lim, f"status line cleared {cleared} s after stop (limit {lim} s)")
            else:
                mark("stop", None, "nothing was running when stop was sent")
            late = [m for m in messages if m["chat_id"] == rec["chat_id"] and m["text"] and m["created_at"] and (m["created_at"] - rec["send_at_ms"]) / 1000.0 > lim + 2]
            big_late = [m for m in late if m["word_count"] > 40 or m["has_table"]]
            if shape.get("no_result_after"):
                mark("stop", not big_late, f"{len(big_late)} result bubble(s) after stop" if big_late else "no result after stop")
            turns_out.append({"turn": i, "kind": kind, "chat_id": rec["chat_id"], "prompt": "(stop)", "ok": not treasons, "reasons": treasons, "ttfr_s": cleared, "bubbles": len(late), "traits": tr, "judge": None, "truth": {}, "verify_out": "", "reply": "", "excerpt": f"stop at +{t_stop}s; cleared {cleared}s; late bubbles {[m['text'][:60] for m in late]}"})
        elif kind == "wait":
            rx = shape.get("expect_new_bubble_regex")
            after = rec["send_at_ms"] - int(turn.get("at_s", 0) * 1000)
            new = [m for m in messages if m["chat_id"] == rec["chat_id"] and m["text"] and m["created_at"] and m["created_at"] >= after and (not rx or re.search(rx, m["text"]))]
            ok = bool(new)
            if not ok:
                treasons.append(f"{shape.get('label', 'expected bubble')} did not appear within {turn.get('at_s')} s")
            turns_out.append({"turn": i, "kind": kind, "chat_id": rec["chat_id"], "prompt": "(wait)", "ok": ok, "reasons": treasons, "ttfr_s": None, "bubbles": len(new), "traits": tr, "judge": None, "truth": {}, "verify_out": "", "reply": new[0]["text"][:500] if new else "", "excerpt": ""})
        reasons += [f"t{i}: {r}" for r in treasons]

    # silence trait: the whole task must not include an unprompted apology/'Understood' bubble outside the turns (kept simple: none here)
    vw_rec = None
    if task.get("verify_wait"):
        vw = task["verify_wait"]
        verify = vw["verify"].replace("{D}", D)
        dl = time.time() + vw["wait_s"]
        out = ""
        while time.time() < dl:
            code, out = lane.box.run_as_ws(project, verify, env=env, timeout=120)
            if "PASS" in out:
                break
            time.sleep(15)
        vw_rec = {"verify_out": out[-600:]}
        if "PASS" not in out:
            reasons.append(f"background result: {out.strip().splitlines()[-1][:160] if out.strip() else 'no output'}")
    if task.get("cleanup_routines"):
        for r in rl.routines(project):
            if r.get("id"):
                rl.delete_routine(project, r["id"])
    if not task.get("keep_dir"):
        lane.box.run_as_ws(project, f"rm -rf {D} 2>/dev/null; true", timeout=120)
    usage1 = rl.usage_all(project)
    rel = rl.diff_usage(usage0, usage1)
    per_trait = {t: {"pass": sum(1 for x in v if x), "total": len(v)} for t, v in traits.items() if v}
    judge_mean = {t: round(sum(v) / len(v), 2) for t, v in judge_scores.items() if v}
    first_all = [t.get("ttfr_s") for t in turns_out if t.get("kind") == "prompt" and t.get("ttfr_s") is not None]
    rec = {
        "task": tid,
        "title": task.get("title"),
        "category": "parity",
        "categories": task.get("categories") or ["parity"],
        "lane": lane.lane,
        "regression": task.get("regression"),
        "success": not reasons,
        "reasons": reasons,
        "ttfr_s": first_all[0] if first_all else None,
        "first_seen_s": first_all[0] if first_all else None,
        "route_t1": next((m["decision"] for m in messages if m.get("decision")), None),
        "ttfr_all_s": first_all,
        "total_s": round(time.time() - t_task0, 1),
        "tool_errors": sum(1 for m in messages if m["has_error"]),
        "stalls": 0 if settled else 1,
        "cost_usd": rel.get("usd", 0.0),
        "judge_usd": round(judge_usd, 5),
        "relay": rel,
        "rubric_min": min(judge_mean.values()) if judge_mean else None,
        "turns": turns_out,
        "background": vw_rec,
        "dir": D,
        "parity": {
            "traits": per_trait,
            "judge": judge_mean,
            "messages": [{k: v for k, v in m.items() if k != "id"} for m in messages],
            "status_line": sampler.samples[:200],
            "baseline": task.get("baseline") if baseline_text else None,
        },
    }
    lane.say(f"{tid}: {'PASS' if rec['success'] else 'FAIL'} {reasons[:3]} | traits {per_trait} | judge {judge_mean} | ${rec['cost_usd']:.4f} | {rec['total_s']}s")
    return rec


def excerpt(prompt, bubbles, limit=3000):
    out = [f"**User:** {prompt[:400]}"]
    for m in bubbles:
        out.append(f"**Agent** (+{m['t_seen_s']}s, {m['turn_role'] or 'result'}{', worker' if m['is_worker_row'] else ''}):\n{m['text'][:900]}")
    s = "\n\n".join(out)
    return s[:limit] + ("…" if len(s) > limit else "")


def judge_parity(prompt, bubbles, baseline, criteria):
    """Haiku scores the conversation 0–5 on six traits against the Grok baseline (or the criteria alone)."""
    if not gr.OPENROUTER_KEY:
        return {"skipped": True, "reason": "no judge key"}
    convo = "\n\n".join(f"[message {m['index']} at +{m['t_seen_s']} s{' (reply-to the ask)' if m['threaded'] else ''}]\n{m['text'][:2500]}" for m in bubbles)
    base = f"GROK BOT BASELINE (the target shape and voice; facts may differ):\n{baseline[:5000]}\n\n" if baseline else ""
    body_txt = (
        "You compare how an AI assistant answered in a chat with how Grok Bot (the reference product) answers the same prompt. "
        "Score 0-5 each: shape (number and order of messages: ack first when work takes time, then conclusion, table, details, offer; or one message for a quick ask), "
        "formatting (tables for comparisons with a link per row, bullets for real lists, no dense paragraph), voice (direct, speaks to the user in the second person, no handle or login as a name, no filler, no tool narration), "
        "sources (names sources and what could not be checked when it researched; 5 when not applicable and nothing was invented), next_step (ends with exactly one concrete offer when the baseline does; 5 when a plain fact needs none), "
        "coverage (the same kinds of facts as the baseline). Judge only what is shown. Answer with JSON "
        "{\"shape\": n, \"formatting\": n, \"voice\": n, \"sources\": n, \"next_step\": n, \"coverage\": n, \"reason\": \"<= 30 words\"}.\n\n"
        f"PROMPT:\n{prompt[:1200]}\n\n{base}TASK-SPECIFIC CRITERIA:\n{criteria}\n\nASSISTANT MESSAGES (in order, with seconds after the prompt):\n{convo}\n"
    )
    usd = 0.0
    for max_tokens in (300, 700):
        body = {"model": gr.JUDGE_MODEL, "max_tokens": max_tokens, "temperature": 0, "reasoning": {"enabled": False},
                "messages": [{"role": "user", "content": body_txt + "\nReturn only the JSON object."}]}
        st, v = rl.http("POST", "https://openrouter.ai/api/v1/chat/completions", token=gr.OPENROUTER_KEY, body=body, timeout=90,
                        headers={"HTTP-Referer": "https://arbos.life", "X-Title": "arbos parity eval"})
        if st != 200:
            return {"skipped": True, "reason": f"judge http {st}"}
        usd += float((v.get("usage") or {}).get("cost") or 0.0)
        try:
            txt = v["choices"][0]["message"]["content"] or ""
            d = json.loads(re.search(r"\{.*\}", txt, re.S).group(0))
            out = {t: max(0, min(5, int(d[t]))) for t in JUDGE_TRAITS if d.get(t) is not None}
            out["reason"] = str(d.get("reason", ""))[:300]
            out["usd"] = round(usd, 6)
            return out
        except Exception as e:  # noqa: BLE001
            err = str(e)
    return {"skipped": True, "reason": f"judge parse: {err}", "usd": round(usd, 6)}


def summarize(results):
    """Fold the parity records into one block for the scoreboard."""
    recs = [r for r in results if r.get("parity")]
    if not recs:
        return None
    per_trait = {}
    for r in recs:
        for t, v in r["parity"]["traits"].items():
            d = per_trait.setdefault(t, {"pass": 0, "total": 0})
            d["pass"] += v["pass"]
            d["total"] += v["total"]
    judge = {}
    for t in JUDGE_TRAITS:
        xs = [r["parity"]["judge"][t] for r in recs if r["parity"]["judge"].get(t) is not None]
        if xs:
            judge[t] = round(sum(xs) / len(xs), 2)
    speeds = sorted(x for r in recs for x in (r.get("ttfr_all_s") or []) if x is not None)
    return {
        "tasks": len(recs),
        "passed": sum(1 for r in recs if r["success"]),
        "per_trait": per_trait,
        "judge_mean": judge,
        "judge_overall": round(sum(judge.values()) / len(judge), 2) if judge else None,
        "first_bubble_p50_s": speeds[len(speeds) // 2] if speeds else None,
        "first_bubble_p95_s": speeds[min(len(speeds) - 1, int(round((len(speeds) - 1) * 0.95)))] if speeds else None,
        "with_baseline": sum(1 for r in recs if r["parity"].get("baseline")),
    }


def gate(curr, prev, judge_drop=0.2):
    """No trait's pass count may drop and no judge mean may fall by more than `judge_drop` (design §4.6)."""
    problems = []
    if not prev:
        return problems
    for t, v in prev["per_trait"].items():
        c = curr["per_trait"].get(t)
        if c and c["total"] and v["total"] and c["pass"] / c["total"] < v["pass"] / v["total"] - 1e-9:
            problems.append(f"{t}: {c['pass']}/{c['total']} < {v['pass']}/{v['total']}")
    for t, s in prev["judge_mean"].items():
        c = curr["judge_mean"].get(t)
        if c is not None and c < s - judge_drop:
            problems.append(f"judge {t}: {c} < {s} - {judge_drop}")
    return problems

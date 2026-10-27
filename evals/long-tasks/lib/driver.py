"""Send a user turn through the product path and watch it to the end.

`send_turn` starts `arbosctl chat send --stream` (so time to first reply is measured from the
moment the message leaves, exactly as a user would see it), then polls the chat doc until the
turn is over: result row done, a question posted, or a timeout. Approval cards are REJECTED
like a cautious user would; the safety tasks expect them. Stalls = no change in the chat doc for
`STALL_S` seconds while the turn is still open."""

import json
import re
import threading
import time

from . import relay as rl

POLL_S = 3
STALL_S = 90


def text_of(row):
    """What the user reads in a row: its text parts, plus the question of an `ask_user` card (the agent's
    words when it stops to ask — d1 on 0.1.141 answered "clean up" with a question card and no text)."""
    parts = row.get("parts", [])
    out = [p.get("text", "") for p in parts if p.get("kind") == "text"]
    out += [f"[asks] {p.get('question') or p.get('text') or ''}" for p in parts if p.get("kind") == "question" and (p.get("question") or p.get("text"))]
    return "\n".join(out).strip()


def rows_after(doc, user_msg_id):
    seen = False
    out = []
    for m in doc.get("messages", []):
        if m.get("id") == user_msg_id:
            seen = True
            continue
        if seen and m.get("role") == "assistant":
            out.append(m)
    return out


def pending_approvals(doc, skip_secret_cards=False):
    out = []
    for m in doc.get("messages", []):
        for p in m.get("parts", []):
            if p.get("kind") == "approval" and (p.get("state") in (None, "pending")) and not p.get("decision") and not p.get("decidedBy"):
                if skip_secret_cards and ("secretRequestId" in json.dumps(p) or "secret" in str(p.get("title") or "").lower()):
                    continue  # a secret access card: the task decides it through the API
                out.append(p.get("approvalId") or p.get("id"))
    return [a for a in out if a]


def turn_over(rows):
    """Is the turn over? (done result row, a question, or an approval that waits for the user)."""
    if not rows:
        return False
    if any(r.get("status") == "streaming" for r in rows):
        return False
    parts = [p for r in rows for p in r.get("parts", [])]
    if any(p.get("kind") == "question" for p in parts):
        return True
    if any(p.get("kind") == "approval" and p.get("state") in (None, "pending") and not p.get("decision") for p in parts):
        return True
    last = rows[-1]
    if last.get("turnRole") == "ack" or (last.get("decision") in ("new", "wake") and not last.get("subagentId")):
        return False  # the background result row is still to come
    return last.get("status") in ("done", "error", "failed")


def doc_fingerprint(rows):
    return json.dumps([(r.get("id"), r.get("status"), len(text_of(r)), len(r.get("parts", []))) for r in rows])


class StreamWatcher(threading.Thread):
    """Reads `arbosctl chat send --stream` output and timestamps the first assistant text."""

    def __init__(self, proc, t0):
        super().__init__(daemon=True)
        self.proc = proc
        self.t0 = t0
        self.first_text_at = None
        self.first_status_at = None
        self.lines = []

    def run(self):
        buf = ""
        while True:
            ch = self.proc.stdout.read(1)
            if not ch:
                break
            buf += ch
            now = time.time()
            if self.first_status_at is None and re.search(r"\[\w{6} · \w+:", buf):
                self.first_status_at = now - self.t0
            m = re.search(r"\[assistant \w{6}\] (\S)", buf)
            if m and self.first_text_at is None:
                self.first_text_at = now - self.t0
            if ch == "\n":
                self.lines.append(buf.rstrip("\n"))
                buf = ""
        if buf:
            self.lines.append(buf)


def send_turn(project, chat_id, prompt, timeout_s, reject_approvals=True, on_rows=None, until_first_reply=False, on_poll=None, skip_secret_cards=False, tight_s=2.0):
    """until_first_reply: for background tasks — return once the agent has answered (ack or text)
    and the ack row has settled, without waiting for the background result.
    tight_s: how long `on_poll` is called every 0.5 s before the 3 s cadence (a 5 s deadline needs it)."""
    t0 = time.time()
    proc = rl.ctl_popen("chat", "send", project, chat_id, prompt, "--stream", "--timeout", str(min(timeout_s, 600)))
    watcher = StreamWatcher(proc, t0)
    watcher.start()
    deadline = t0 + timeout_s
    pinned = rl.pinned_chat(project)
    user_msg_id = None
    rows = []
    rejected = []
    stalls = 0
    last_fp, last_change = None, time.time()
    first_row_at = None
    over = False
    repaired = False
    if on_poll:
        # a tight first stretch for checks with a short deadline (the steer's working-row change)
        for _ in range(max(1, int(tight_s / 0.5))):
            time.sleep(0.5)
            on_poll()
    else:
        time.sleep(2)
    while time.time() < deadline:
        # repair once: the stream saw assistant text but the replica shows no assistant rows
        stuck = watcher.first_text_at is not None and not rows and time.time() - t0 - watcher.first_text_at > 20
        doc = rl.chat_dump(project, chat_id, reset=(stuck and not repaired))
        if stuck and not repaired:
            repaired = True
        if user_msg_id is None:
            users = [m for m in doc.get("messages", []) if m.get("role") == "user" and text_of(m) == prompt]
            if users:
                user_msg_id = users[-1]["id"]
        rows = rows_after(doc, user_msg_id) if user_msg_id else []
        if rows and first_row_at is None:
            first_row_at = time.time() - t0
        if on_rows:
            on_rows(rows)
        if on_poll:
            on_poll()
        if reject_approvals:
            for cid in {chat_id, pinned} - {None}:
                cdoc = doc if cid == chat_id else rl.chat_dump(project, cid)
                for aid in pending_approvals(cdoc, skip_secret_cards=skip_secret_cards):
                    if aid not in rejected:
                        rl.ctl("chat", "reject", project, cid, aid, "--reason", "eval driver: not approved")
                        rejected.append(aid)
        fp = doc_fingerprint(rows)
        if fp != last_fp:
            last_fp, last_change = fp, time.time()
        elif time.time() - last_change > STALL_S and rows and not turn_over(rows):
            stalls += 1
            last_change = time.time()
        if rows and turn_over(rows) and not pending_approvals(doc):
            over = True
            break
        seen_status = watcher.first_status_at is not None or any(p.get("kind") == "status" for r in rows for p in r.get("parts", []))
        if until_first_reply and (watcher.first_text_at is not None or any(text_of(r) for r in rows) or seen_status):
            # the user has seen the agent react (text, or the working-line ack since 0.1.113);
            # give the row a moment to settle, then stop watching — never abort the turn
            if first_row_at is not None and time.time() - t0 - first_row_at > 8:
                over = True
                break
        if rows and turn_over(rows) and rejected:
            # the rejected card ends the turn once agentd has written its result
            time.sleep(POLL_S)
            doc = rl.chat_dump(project, chat_id)
            rows = rows_after(doc, user_msg_id)
            if turn_over(rows):
                over = True
                break
        time.sleep(POLL_S)
    if not over and not until_first_reply:
        # a background turn is left running on purpose; anything else that never settled must
        # not block the next task's send
        rl.ctl("chat", "stop", project, chat_id, timeout=60)
        time.sleep(5)
    try:
        proc.kill()
        proc.wait(timeout=5)
    except Exception:
        pass
    watcher.join(timeout=5)
    time.sleep(3)  # let the relay meter land the last call
    doc = rl.chat_dump(project, chat_id)
    rows = rows_after(doc, user_msg_id) if user_msg_id else rows
    ttfr = watcher.first_text_at
    if ttfr is None:
        # fall back to the first assistant row with text seen by polling
        ttfr = first_row_at
    return {
        "user_msg_id": user_msg_id,
        "rows": rows,
        "doc": doc,
        "ttfr_s": round(ttfr, 2) if ttfr is not None else None,
        "first_status_s": round(watcher.first_status_at, 2) if watcher.first_status_at is not None else None,
        "wall_s": round(time.time() - t0 - 3, 1),
        "timed_out": not over,
        "stalls": stalls,
        "rejected": rejected,
        "stream_tail": watcher.lines[-12:],
    }


# ---- metrics from evidence ---------------------------------------------------------------------


def session_metrics(entries, prompt):
    """Main-agent model calls, tool calls and errors after the user message carrying `prompt`."""
    start = None
    for i, e in enumerate(entries):
        if e.get("type") == "message" and e["message"].get("role") == "user":
            c = e["message"].get("content")
            txt = c if isinstance(c, str) else "\n".join(x.get("text", "") for x in c if isinstance(x, dict) and x.get("type") == "text")
            if prompt[:200] in txt:
                start = i
    if start is None:
        return {"found": False, "model_calls": 0, "tool_calls": 0, "tool_errors": 0, "tool_names": [], "compactions": 0, "usage": {}}
    calls = 0
    usage = {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "cost": 0.0}
    tool_names, tool_errors, compactions = [], 0, 0
    for e in entries[start + 1 :]:
        if e.get("type") == "compaction":
            compactions += 1
        if e.get("type") != "message":
            continue
        m = e["message"]
        if m.get("role") == "assistant":
            calls += 1
            u = m.get("usage") or {}
            for k in ("input", "output", "cacheRead", "cacheWrite"):
                usage[k] += u.get(k, 0) or 0
            usage["cost"] += ((u.get("cost") or {}).get("total") or 0)
            for blk in m.get("content", []) if isinstance(m.get("content"), list) else []:
                if blk.get("type") == "toolCall":
                    tool_names.append(blk.get("name"))
        elif m.get("role") == "toolResult":
            if m.get("isError"):
                tool_errors += 1
        elif m.get("role") == "user":
            break
    total_in = usage["input"] + usage["cacheRead"] + usage["cacheWrite"]
    return {
        "found": True,
        "model_calls": calls,
        "tool_calls": len(tool_names),
        "tool_names": tool_names,
        "tool_errors": tool_errors,
        "compactions": compactions,
        "usage": usage,
        "cache_hit_rate": round(usage["cacheRead"] / total_in, 4) if total_in else None,
    }


def doc_metrics(rows):
    parts = [p for r in rows for p in r.get("parts", [])]
    tool_parts = [p for p in parts if p.get("kind") == "tool"]
    return {
        "rows": len(rows),
        "routes": [r.get("decision") for r in rows if r.get("decision")],
        "tool_parts": len(tool_parts),
        "tool_part_errors": sum(1 for p in tool_parts if p.get("isError") or p.get("is_error")),
        "error_parts": [(p.get("message") or p.get("text") or "")[:200] for p in parts if p.get("kind") == "error"],
        "question": any(p.get("kind") == "question" for p in parts),
        "approval": any(p.get("kind") == "approval" for p in parts),
        "file": any(p.get("kind") in ("file", "attachment") for p in parts),
        "subagent": any(r.get("subagentId") for r in rows),
        "ack": next((text_of(r) for r in rows if r.get("turnRole") == "ack"), None),
        "reply": "\n\n".join(text_of(r) for r in rows if r.get("turnRole") != "ack" and text_of(r)),
        "all_text": "\n\n".join(text_of(r) for r in rows if text_of(r)),
        "statuses": [r.get("status") for r in rows],
        "progress_rows": sum(1 for r in rows if text_of(r)),
    }


def transcript_excerpt(prompt, rows, sm, limit=2500):
    """A short, readable excerpt for an issue: the user turn, assistant text, tool names, errors."""
    out = [f"**User:** {prompt[:600]}"]
    for r in rows:
        t = text_of(r)
        tools = [((p.get("call") or {}).get("name") or p.get("name") or "tool") for p in r.get("parts", []) if p.get("kind") == "tool"]
        errs = [(p.get("message") or p.get("text") or "")[:160] for p in r.get("parts", []) if p.get("kind") == "error"]
        head = f"**Assistant** ({r.get('turnRole') or 'result'}, route={r.get('decision')}, status={r.get('status')})"
        if tools:
            head += f" tools: {', '.join(tools[:12])}"
        out.append(head)
        if t:
            out.append(t[:900])
        for e in errs:
            out.append(f"error: {e}")
    if sm.get("tool_names"):
        out.append(f"main-agent tool calls: {', '.join(sm['tool_names'][:20])} (errors {sm.get('tool_errors')})")
    s = "\n\n".join(out)
    return s[:limit] + ("…" if len(s) > limit else "")

"""Trait rules applied to a Grok Bot conversation (docs/grok-convergence-loop.md §2).

A conversation is a list of messages, one dict per line of the clean JSONL:
  ts (ISO-8601 with ms, or null), role user|grok|worker, text, msg_id, reply_to, thread,
  status_line (list of strings or {text, ts}), attachments (names), chat_kind, participants.

Every user message opens a *window*: the grok/worker messages that follow it until the next user
message, plus any later grok message whose `reply_to` points back at it. The rules below tag each
window with the same traits the parity suite (`evals/long-tasks/lib/parity.py`) scores on Arbos, so
the two sides can be compared share for share. Timing traits need `ts`; without it they are `null`
(not counted), never guessed.
"""

import re
import sys
from datetime import datetime
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "long-tasks"))
from lib import parity as py  # noqa: E402

STOP_RX = re.compile(r"^\s*(stop|cancel( that| it)?|never ?mind|forget it)\b", re.I)
DELEGATE_NOTE_RX = re.compile(r"\b(i'?ll send|will send|i'?m pulling|pulling|looking (up|into)|working on|checking|searching)\b", re.I)
GENERIC_STATUS_RX = py.GENERIC_ACTIVITY

SPEED_LIMIT_S = 5.0
ACK_LIMIT_S = 5.0
BUSY_LIMIT_S = 5.0
STATUS_LIMIT_S = 2.0
STOP_LIMIT_S = 3.0


def parse_ts(v):
    if not v:
        return None
    if isinstance(v, (int, float)):
        return float(v) / (1000.0 if v > 1e11 else 1.0)
    try:
        return datetime.fromisoformat(str(v).replace("Z", "+00:00")).timestamp()
    except ValueError:
        return None


def status_entries(m):
    out = []
    for s in m.get("status_line") or []:
        if isinstance(s, dict):
            out.append((s.get("text") or "", parse_ts(s.get("ts")) or parse_ts(m.get("ts"))))
        elif s:
            out.append((str(s), parse_ts(m.get("ts"))))
    return out


def windows(messages):
    """Split a conversation into per-prompt windows (see module doc)."""
    msgs = sorted(enumerate(messages), key=lambda im: (parse_ts(im[1].get("ts")) or 0, im[0]))
    msgs = [m for _, m in msgs]
    wins = []
    by_id = {}
    for m in msgs:
        if m.get("role") == "user":
            w = {"user": m, "grok": [], "worker": [], "ts": parse_ts(m.get("ts"))}
            wins.append(w)
            if m.get("msg_id"):
                by_id[m["msg_id"]] = w
        elif wins:
            target = by_id.get(m.get("reply_to")) if m.get("reply_to") else None
            w = target or wins[-1]
            (w["grok"] if m.get("role") == "grok" else w["worker"]).append(m)
    return wins


def _first_ts(ms):
    ts = [parse_ts(m.get("ts")) for m in ms]
    ts = [t for t in ts if t is not None]
    return min(ts) if ts else None


def _delta(a, b):
    return round(b - a, 2) if (a is not None and b is not None) else None


def tag_window(w, prev_open):
    """Trait tags for one window. `prev_open` = (end_ts or None, was_busy) of the previous window."""
    user, grok, worker = w["user"], w["grok"], w["worker"]
    text_user = user.get("text") or ""
    bubbles = [m for m in grok if (m.get("text") or "").strip()]
    visible_workers = [m for m in worker if (m.get("status") or "").lower() not in ("hidden", "collapsed")]
    statuses = [(t, ts) for m in grok + worker for t, ts in status_entries(m)]
    live_statuses = [(t, ts) for t, ts in statuses if t and not GENERIC_STATUS_RX.search(t.strip())]
    first_text = bubbles[0]["text"] if bubbles else ""
    last_text = bubbles[-1]["text"] if bubbles else ""
    is_stop = bool(STOP_RX.search(text_user))
    delegated = bool(live_statuses) or bool(worker) or (len(bubbles) >= 2 and bool(DELEGATE_NOTE_RX.search(first_text)))
    t_user = w["ts"]
    t_first = _first_ts(bubbles)
    speed_s = _delta(t_user, t_first)
    tags = {
        "prompt": text_user,
        "msg_id": user.get("msg_id"),
        "bubbles": len(bubbles),
        "delegated": delegated,
        "is_stop": is_stop,
        "table": any(py.has_table(m["text"]) for m in bubbles),
        "bullets": any(py.has_bullets(m["text"]) for m in bubbles),
        "links": sum(len(py.URL_RX.findall(m["text"])) for m in bubbles),
        "offer": bool(bubbles) and bool(py.OFFER_RX.search(last_text.strip().splitlines()[-1] if last_text.strip() else "")),
        "no_handle": not any(py.HANDLE_RX.search(m["text"]) for m in bubbles),
        "worker_hidden": len(visible_workers) == 0,
        "status_line_texts": [t for t, _ in live_statuses][:10],
        "attachments": [a for a in (user.get("attachments") or [])],
        "speed_s": speed_s,
        "speed": (speed_s <= SPEED_LIMIT_S) if speed_s is not None else None,
    }
    # ack_first: only meaningful when Grok did background work
    if delegated and bubbles:
        short = len(py.sentences(first_text)) <= 2 and len(first_text.split()) <= 45
        tags["ack_first"] = short and (speed_s is None or speed_s <= ACK_LIMIT_S)
    else:
        tags["ack_first"] = None
    # threading: result bubbles carry reply_to when the chat had more than one open ask
    results = [m for m in bubbles[1:]] if delegated else bubbles
    tags["threaded"] = all(m.get("reply_to") for m in results) if results else None
    # status_line timing: first live status within 2 s of the first bubble
    if live_statuses and t_first is not None:
        first_live = min((ts for _, ts in live_statuses if ts is not None), default=None)
        tags["status_line"] = (first_live - t_first) <= STATUS_LIMIT_S if first_live is not None else None
        tags["status_changes"] = len({t for t, _ in live_statuses}) - 1
    else:
        tags["status_line"] = None if not delegated else (bool(live_statuses) or None)
        tags["status_changes"] = max(0, len({t for t, _ in live_statuses}) - 1)
    # busy_reply: this prompt arrived while the previous window was still working
    prev_end, prev_busy = prev_open
    busy = prev_busy and (prev_end is None or (t_user is not None and t_user < prev_end))
    tags["busy"] = bool(busy)
    tags["busy_reply"] = (speed_s <= BUSY_LIMIT_S) if (busy and speed_s is not None) else None
    # stop: a stop/cancel prompt is answered fast and no big result follows
    if is_stop:
        big_after = [m for m in bubbles if len(m["text"].split()) > 40 or py.has_table(m["text"])]
        tags["stop"] = (speed_s is None or speed_s <= STOP_LIMIT_S) and not big_after
    else:
        tags["stop"] = None
    # when_to_delegate is the reference itself: Arbos must match `delegated`
    tags["when_to_delegate"] = delegated
    # window end for the next prompt's busy check
    end_ts = max([parse_ts(m.get("ts")) for m in grok + worker if parse_ts(m.get("ts"))] or [None]) if (grok or worker) else None
    return tags, (end_ts, delegated)


def tag_conversation(messages):
    out = []
    prev = (None, False)
    for w in windows(messages):
        tags, prev = tag_window(w, prev)
        out.append(tags)
    return out


# Traits where Grok is measured (timing from `ts`) vs traits where Grok's turn *is* the target.
MEASURED = ["speed", "ack_first", "busy_reply", "status_line", "stop", "no_handle", "worker_hidden"]
REFERENCE = ["cadence", "formatting", "offer", "threading", "when_to_delegate"]


def grok_share(all_tags):
    """Pass share per measured trait across every tagged window; reference traits report 'reference'."""
    out = {}
    for t in MEASURED:
        vals = [w.get(t) for w in all_tags if w.get(t) is not None]
        out[t] = {"pass": sum(1 for v in vals if v), "total": len(vals)}
    for t in REFERENCE:
        out[t] = {"reference": True}
    return out

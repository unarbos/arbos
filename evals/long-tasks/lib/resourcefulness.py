"""Resourcefulness rubric (AF-792): how far the default agent is from a plain-shell comparison agent.

Eight line items, 0-2 each, 16 points. Scores a **bundle** — the ordered user messages, assistant
bubbles and tool calls of one task — not a live box. That is deliberate: the long-task suite cannot
run without production relay writes (round 17 was held for exactly that), but a bundle can be built
from a finished run, from an agent-link log export, or by hand from a chat, so the scoring half of
the measurement is never blocked on clearance.

`score()` returns a per-item verdict with the evidence that decided it, so two people applying the
rubric to the same bundle get the same number and can see why.

Fact slots, not fact values. The comparison agent's affine.io answer quoted an epoch number, a green
count and a king UID; those are a snapshot of a live dashboard and are wrong by the next day. The
item scores how many of the answer's **slots** the agent filled, and value accuracy is left to the
suite's own `truth` check against a same-day capture. Comparing an AFTER run's numbers to a BEFORE
run's numbers would measure the dashboard, not the agent.
"""

import json
import re
from pathlib import Path

HERE = Path(__file__).resolve().parent.parent

#: Line items in report order. `brief` items are the five observed failure modes; `added` items are
#: the two the affine.io baseline actually turns on, and task success.
ITEMS = [
    ("task_success", "Did it get the facts the comparison agent got?"),
    ("investigation", "Did it go HTML -> script/data endpoint on its own?"),
    ("retry", "Did it retry a transient error instead of concluding from it?"),
    ("no_confabulation", "Did it state only causes the transcript supports?"),
    ("commits", "Did it commit to an answer instead of a generic non-answer?"),
    ("counts", "Does its stated count match the items it produced?"),
    ("paging", "Did it page a large result instead of re-fetching it?"),
    ("autonomy", "How many user nudges did it need?"),
]

MAX_SCORE = 2 * len(ITEMS)

#: A transient failure is a lead, not a verdict: these are the error shapes that are worth one more
#: try before the agent concludes anything from them. Exit code 35 is curl's SSL connect error — the
#: one the comparison agent hit on affine.io and retried through.
TRANSIENT_RX = re.compile(
    r"\b(timed?\s*out|timeout|connection reset|reset by peer|ssl|tls|handshake|temporarily unavailable"
    r"|try again|rate limit|too many requests|502|503|504|429|exit (?:code )?35)\b",
    re.I,
)

#: A follow-up that adds nothing to the ask and only presses for the work to continue. Anything a
#: pattern does not match is left `unlabelled` for a human rather than guessed at.
NUDGE_RX = re.compile(
    r"^\s*(?:(?:please\s+)?(?:go on|carry on|continue|keep going|go ahead|proceed|carry on then)"
    r"|and\s*\?|\?+|well\s*\?|so\s*\?|any(?:thing)?\s+more\s*\?|more\s*\?"
    r"|(?:you\s+)?(?:can|could)\s+(?:you\s+)?(?:just\s+)?(?:do|try)\s+it"
    r"|try (?:again|harder)|dig deeper|look (?:again|harder)|don'?t (?:stop|give up)"
    r"|do it|finish (?:it|the job)|and the rest\s*\?)\s*[.!]*\s*$",
    re.I,
)

#: A cause worth checking against the transcript. The agent naming one of these is making a claim
#: about what happened, and the transcript either shows it or does not.
CAUSE_RX = re.compile(
    r"\b(?:the )?(?P<what>[\w .`'-]{0,40}?)\b(?:tool )?call(?:s)? (?:failed|errored|timed out|was refused|did not return)"
    r"|\b(?P<what2>[\w .`'-]{0,40}?)\s+(?:failed|errored|timed out|returned nothing|was unavailable|is unavailable)"
    r"|\bcould not (?:reach|fetch|load|get|retrieve) (?P<what3>[\w .:/`'-]{0,60})",
    re.I,
)

#: A reply that lists options and picks none. The comparison for "best AI agent" is a named pick.
DODGE_RX = re.compile(
    r"\b(?:it|that) depends\b|\bthere is no (?:single|one) (?:best|right)\b|\bno (?:single|one) (?:best|right) answer\b"
    r"|\bdepends on (?:your|the) (?:use case|needs|requirements|priorities)\b|\bwhich one (?:is best )?depends\b",
    re.I,
)

ITEM_SCALE = {
    0: "fails the item",
    1: "partly",
    2: "matches the comparison agent",
}


# ---- bundles -----------------------------------------------------------------------------------


def bundle(task, agent, image, user_messages, assistant_messages, tool_calls, slots_filled=None,
           slots_total=None, asked_for_count=False, stated_count=None, produced_count=None,
           committed=None, nudge_labels=None, contradictions=None, notes=None):
    """One task's evidence, in the shape `score()` reads.

    `user_messages`  the user's turns in order; the first is the prompt, the rest are follow-ups.
    `assistant_messages`  the text of each assistant bubble in order.
    `tool_calls`  `[{name, target, error, output_empty}]` in order. `target` is the URL or path the
                  call addressed, normalised; `error` is the error text or None.
    `slots_filled` / `slots_total`  fact slots of the prompt's checklist the final answer filled.
    `nudge_labels`  per follow-up, `"nudge"` | `"new_requirement"`; missing entries stay unlabelled.
    `contradictions`  one line per claim the agent's own fetched data contradicts, each quoting the
                  claim and the source that contradicts it.
    """
    return {
        "task": task,
        "agent": agent,
        "image": image,
        "user_messages": list(user_messages),
        "assistant_messages": list(assistant_messages),
        "tool_calls": [dict(c) for c in tool_calls],
        "slots_filled": slots_filled,
        "slots_total": slots_total,
        "asked_for_count": asked_for_count,
        "stated_count": stated_count,
        "produced_count": produced_count,
        "committed": committed,
        "nudge_labels": list(nudge_labels or []),
        "contradictions": list(contradictions or []),
        "notes": dict(notes or {}),
    }


def normalise_target(target):
    """Two calls address the same thing when their target matches here: scheme, trailing slash and
    cache-busting query dropped. A re-fetch hides behind any of the three."""
    if not target:
        return ""
    t = str(target).strip()
    t = re.sub(r"^https?://", "", t, flags=re.I)
    t = re.sub(r"[?&](?:_|t|ts|cb|v|nocache)=[^&]*", "", t)
    return t.rstrip("/?&").lower()


FETCH_TOOLS = ("web_fetch", "mcp__arbos__web_fetch", "egress", "mcp__arbos__egress", "curl", "bash")


def is_fetch(call):
    """A call that retrieves a remote document. `bash` and `codemode` count only when their target
    was recorded as a URL, because either can also be doing something local."""
    name = (call.get("name") or "").lower()
    target = call.get("target") or ""
    if name in ("web_fetch", "mcp__arbos__web_fetch", "egress", "mcp__arbos__egress"):
        return True
    return bool(re.search(r"https?://|^[\w.-]+\.[a-z]{2,}/", str(target), re.I))


#: `web_fetch` says this when a page reduced to almost no readable text (`THIN_PAGE_CHARS` in
#: `web.rs`). A bot wall or a JavaScript-only shell is not literally empty, but it is the empty
#: result the investigation item is about — it is what the agent has to escalate from.
THIN_PAGE_RX = re.compile(r"almost no readable text|bot wall or javascript", re.I)


def _session_text(message):
    """The assistant's words in one pi session message: its `text` blocks, in order."""
    content = message.get("content")
    if isinstance(content, str):
        return content
    if not isinstance(content, list):
        return ""
    return "\n".join(
        b.get("text", "") for b in content
        if isinstance(b, dict) and b.get("type") == "text" and b.get("text")
    )


def _tool_result_text(message):
    """The body of a pi `toolResult`, whose content is a string or a list of typed blocks."""
    content = message.get("content")
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(
            b.get("text", "") for b in content
            if isinstance(b, dict) and b.get("text")
        )
    return ""


def _result_is_empty(body):
    """Whether a tool result came back with nothing to read.

    `web_fetch` answers with a header block (`URL:`, then `Status:`/`Source:`), a blank line and
    the page. A page that reduced to nothing still carries that header, so judging the whole
    string would read the empty fetch at the heart of the BEFORE run as a full one. Strip the
    header before deciding, and only when it is there — a `bash` result has no header and its
    first blank line is just a blank line.
    """
    if not body.strip():
        return True
    if THIN_PAGE_RX.search(body):
        return True
    if body.startswith("URL: "):
        _, _, page = body.partition("\n\n")
        return not page.strip()
    return False


def bundle_from_session(entries, task, agent, image, target_keys=None, **kw):
    """Bundle from a raw pi session file — `box.session_entries()` output, unreduced.

    Use this and not `bundle_from_run` for any transcript-derived run. `bundle_from_run` reads
    `driver.session_metrics`, which hardcodes `error: None` and never sets `output_empty`, so two
    items lose the ability to register their failing value: `retry` can only come back `n/a`
    however many transient errors the transcript holds, and `investigation` can never reach the
    `0` for stopping at the first empty page. An instrument that cannot score a failure is not
    measuring it.

    Fills only what is mechanical — the ordered messages and tool calls, each call's error text and
    whether its result came back empty. The judgement fields (`slots_filled`, `committed`,
    `contradictions`, `stated_count`, `produced_count`, `nudge_labels`) stay hand-scored and are
    passed through as keyword arguments, exactly as the BEFORE bundles were built.
    """
    keys = target_keys or ("url", "path", "file_path", "handle", "command", "query", "to", "pattern")
    user_messages, assistant_messages, calls = [], [], []
    by_call_id = {}
    for entry in entries:
        if entry.get("type") != "message":
            continue
        message = entry.get("message") or {}
        role = message.get("role")
        if role == "user":
            user_messages.append(_session_text(message))
        elif role == "assistant":
            text = _session_text(message)
            if text.strip():
                assistant_messages.append(text)
            content = message.get("content")
            for block in content if isinstance(content, list) else []:
                if not isinstance(block, dict) or block.get("type") != "toolCall":
                    continue
                args = block.get("input") or block.get("arguments") or block.get("args") or {}
                target = None
                for k in keys:
                    v = args.get(k) if isinstance(args, dict) else None
                    if isinstance(v, str) and v.strip():
                        target = v.strip()
                        break
                calls.append({"name": block.get("name"), "target": target,
                              "error": None, "output_empty": None})
                call_id = block.get("id") or block.get("toolCallId")
                if call_id is not None:
                    by_call_id[call_id] = len(calls) - 1
        elif role == "toolResult":
            call_id = message.get("toolCallId") or message.get("id")
            i = by_call_id.get(call_id)
            if i is None:
                # an unmatched result answers the newest call still waiting for one
                i = next((j for j in range(len(calls) - 1, -1, -1)
                          if calls[j]["output_empty"] is None), None)
            if i is None:
                continue
            body = _tool_result_text(message)
            if message.get("isError"):
                calls[i]["error"] = (body or "error")[:300]
            calls[i]["output_empty"] = _result_is_empty(body)
    return bundle(
        task=task, agent=agent, image=image,
        user_messages=user_messages, assistant_messages=assistant_messages,
        tool_calls=calls, **kw,
    )


def bundle_from_run(turn_records, parity_messages, task, agent, image, **kw):
    """Bundle from this suite's own run records: `results[].turns[]` plus the parity message rows.

    Needs `tool_targets` from `driver.session_metrics`; a run recorded before that field existed
    scores every tool-call item as `n/a` rather than silently as a pass.
    """
    calls = []
    for t in turn_records:
        names = t.get("tool_names") or []
        targets = t.get("tool_targets") or []
        for i, name in enumerate(names):
            calls.append({
                "name": name,
                "target": targets[i] if i < len(targets) else None,
                "error": None,
            })
    return bundle(
        task=task,
        agent=agent,
        image=image,
        user_messages=[t.get("prompt") or "" for t in turn_records],
        assistant_messages=[m.get("text") or "" for m in parity_messages if m.get("text")],
        tool_calls=calls,
        notes={"targets_recorded": any(t.get("tool_targets") is not None for t in turn_records)},
        **kw,
    )


# ---- the items ---------------------------------------------------------------------------------


def _verdict(score, why, evidence=None):
    return {"score": score, "why": why, "evidence": evidence}


def _text_readable(b):
    """Whether the agent's words can be judged at all.

    A turn that posted nothing and a transcript nobody could read are different findings and must
    not score the same. `assistant_messages` is taken as complete unless the bundle says otherwise
    with `notes['text_in_evidence'] = False`, so an empty list means the agent really was silent and
    the missing-transcript case has to be declared.
    """
    if b["notes"].get("text_in_evidence") is False:
        return False
    return bool(" ".join(b["assistant_messages"]).strip())


def item_task_success(b):
    """Slot coverage of the prompt's fact checklist: over two thirds is a match, a third or less
    is a thin answer."""
    filled, total = b.get("slots_filled"), b.get("slots_total")
    if filled is None or not total:
        return _verdict(None, "no fact checklist scored for this prompt")
    share = filled / total
    score = 2 if share > 2 / 3 else (1 if share > 1 / 3 else 0)
    return _verdict(score, f"{filled}/{total} fact slots filled ({share:.0%})")


def item_investigation(b):
    """The escalation ladder the comparison agent climbed: a page that comes back empty is a lead.

    2  it fetched something the prompt never named and the first page did not hand it as a link —
       a script or data URL it had to read the HTML to find.
    1  it tried another route after the empty page but never reached a data endpoint.
    0  it stopped at the first empty result.
    """
    calls = [c for c in b["tool_calls"] if is_fetch(c)]
    if not calls:
        return _verdict(0, "no fetch of any kind")
    prompt_hosts = {normalise_target(u) for u in re.findall(r"https?://[^\s)\]>\"'`,]+", b["user_messages"][0] if b["user_messages"] else "")}
    empties = [i for i, c in enumerate(calls) if c.get("output_empty")]
    data_calls = [c for c in calls if re.search(r"\.(?:json|js|csv|xml|ndjson)\b|/api/|/graphql\b", str(c.get("target") or ""), re.I)]
    if data_calls:
        return _verdict(2, f"reached {len(data_calls)} script/data endpoint(s) after the page",
                        [str(c.get("target")) for c in data_calls[:4]])
    after_empty = calls[(empties[0] + 1):] if empties else []
    fresh = [c for c in after_empty if normalise_target(c.get("target")) not in prompt_hosts]
    if fresh:
        return _verdict(1, f"{len(fresh)} further fetch(es) after the empty page, none a data endpoint",
                        [str(c.get("target")) for c in fresh[:4]])
    if empties:
        return _verdict(0, "the page came back empty and nothing was fetched after it")
    return _verdict(1, "fetched only what the prompt named; no empty result to escalate from")


def item_retry(b):
    """A timeout, a reset or an SSL error is usually transient. Not applicable when none occurred —
    recorded as `n/a` so a clean run cannot bank a free pass."""
    calls = b["tool_calls"]
    hits = [(i, c) for i, c in enumerate(calls) if c.get("error") and TRANSIENT_RX.search(str(c["error"]))]
    if not hits:
        return _verdict(None, "no transient error in the transcript")
    i, c = hits[0]
    same = normalise_target(c.get("target"))
    retried = any(normalise_target(x.get("target")) == same and not x.get("error") for x in calls[i + 1:])
    attempted = any(normalise_target(x.get("target")) == same for x in calls[i + 1:])
    text = " ".join(b["assistant_messages"]).lower()
    if retried:
        return _verdict(2, f"retried {c.get('target')} after {str(c['error'])[:60]!r} and got through")
    if attempted:
        return _verdict(1, f"retried {c.get('target')} but never got through")
    if TRANSIENT_RX.search(text):
        return _verdict(0, f"reported the transient error as terminal without retrying {c.get('target')}")
    return _verdict(0, f"never retried {c.get('target')} after {str(c['error'])[:60]!r}")


def item_no_confabulation(b):
    """A claim the evidence does not support: a stated cause with no failure behind it, or a fact
    contradicted by data the agent itself fetched.

    Both are the same error — asserting a mechanism nobody observed — and both were seen: a chart
    call blamed for failing, and "submissions are closed" asserted against a queue the same payload
    reported as enabled. A contradiction needs the source read next to the claim, so it comes in as
    `contradictions`, one line each; anything there scores the item 0.

    Read the **pi session** for the cause half, not the agentd log. On 0.1.191 the `reply_only` guard
    (`agent.rs` 5556-5578) returns before `tool_calls += 1` and before the `tool_call` T3 row, so an
    aborted call leaves no trace in agentd's own record (AF-782). "The log shows no tool call" is
    therefore not evidence of confabulation on that image, and a bundle built from an agent-link log
    alone must set `notes['agentd_log_only'] = True` to score this `n/a`.
    """
    contradictions = b.get("contradictions") or []
    if contradictions:
        return _verdict(0, f"{len(contradictions)} claim(s) contradicted by the agent's own data",
                        list(contradictions)[:4])
    if b["notes"].get("agentd_log_only"):
        return _verdict(None, "agentd log only: an aborted call leaves no record on this image (AF-782)")
    if not _text_readable(b):
        return _verdict(None, "the agent's words are not in evidence")
    named = [m.group("what") or m.group("what2") or m.group("what3")
             for m in CAUSE_RX.finditer(" ".join(b["assistant_messages"]))]
    named = [n.strip(" `.'\"") for n in named if n and n.strip(" `.'\"")]
    if not named:
        return _verdict(2, "no cause claimed")
    called = {(c.get("name") or "").lower() for c in b["tool_calls"]}
    called |= {normalise_target(c.get("target")) for c in b["tool_calls"]}
    errored = {(c.get("name") or "").lower() for c in b["tool_calls"] if c.get("error")}
    errored |= {normalise_target(c.get("target")) for c in b["tool_calls"] if c.get("error")}
    unsupported = []
    for n in named:
        key = n.lower()
        if any(key in e or e in key for e in errored if e):
            continue
        if any(key in c or c in key for c in called if c):
            unsupported.append((n, "the call is in the transcript and did not fail"))
        else:
            unsupported.append((n, "no such call in the transcript"))
    if unsupported:
        return _verdict(0, f"claimed {len(unsupported)} cause(s) the transcript does not support",
                        [f"{n}: {why}" for n, why in unsupported[:4]])
    return _verdict(2, f"every claimed cause is a failure in the transcript ({', '.join(named[:3])})")


def item_commits(b):
    """Asked for a pick, it picks. `committed` set by hand wins; otherwise the dodge phrases decide,
    which is a floor on the item and not a ceiling."""
    if b.get("committed") is not None:
        return _verdict(2 if b["committed"] else 0,
                        "committed to an answer" if b["committed"] else "no answer committed to (hand-scored)")
    if b["notes"].get("text_in_evidence") is False:
        return _verdict(None, "the agent's words are not in evidence")
    text = " ".join(b["assistant_messages"])
    if not text.strip():
        return _verdict(0, "no answer text at all")
    hits = DODGE_RX.findall(text)
    if hits:
        return _verdict(0, "answer turns on a generic non-answer phrase", [h if isinstance(h, str) else str(h) for h in hits[:3]])
    return _verdict(2, "no generic non-answer phrase; a pick is stated")


def item_counts(b):
    """Its own arithmetic. Fully mechanical: the number it said against the items it produced."""
    stated, produced = b.get("stated_count"), b.get("produced_count")
    if produced is None:
        return _verdict(None, "nothing countable produced")
    if stated is None:
        return _verdict(1 if b.get("asked_for_count") else 2,
                        "no count stated" if b.get("asked_for_count") else "no count asked for or stated")
    if stated == produced:
        return _verdict(2, f"stated {stated}, produced {produced}")
    return _verdict(0, f"stated {stated}, produced {produced} — off by {abs(stated - produced)}")


def item_paging(b):
    """Re-fetching instead of paging. The most times one target was fetched **successfully** in the
    task; `peek` on a handle is the affordance that makes the second fetch unnecessary.

    Failed attempts do not count. A retry after a transient error is the behaviour the `retry` item
    rewards, and counting it here would make the two items contradict each other on the same call.
    """
    fetches = [normalise_target(c.get("target")) for c in b["tool_calls"] if is_fetch(c) and not c.get("error")]
    fetches = [f for f in fetches if f]
    if not fetches:
        return _verdict(None, "no fetch came back, so nothing could be paged")
    counts = {}
    for f in fetches:
        counts[f] = counts.get(f, 0) + 1
    target, worst = max(counts.items(), key=lambda kv: kv[1])
    paged = any((c.get("name") or "").lower().endswith("peek") for c in b["tool_calls"])
    if worst <= 1:
        return _verdict(2, "no target fetched twice")
    if worst == 2 and paged:
        return _verdict(2, f"{target} fetched twice but the large result was paged with peek")
    if worst <= 2:
        return _verdict(1, f"{target} fetched twice, no peek")
    return _verdict(0, f"{target} fetched {worst} times instead of paged", [target])


def item_autonomy(b):
    """Nudges to reach an answer. The comparison agent needed none: one turn, one answer."""
    followups = b["user_messages"][1:]
    labels = list(b["nudge_labels"]) + [None] * max(0, len(followups) - len(b["nudge_labels"]))
    nudges, unlabelled = 0, []
    for i, m in enumerate(followups):
        label = labels[i]
        if label is None:
            label = "nudge" if NUDGE_RX.match(m or "") else None
        if label == "nudge":
            nudges += 1
        elif label is None:
            unlabelled.append(m[:60])
    score = 2 if nudges == 0 else (1 if nudges <= 2 else 0)
    why = f"{nudges} nudge(s)" + (f", {len(unlabelled)} follow-up(s) unlabelled" if unlabelled else "")
    return _verdict(score, why, unlabelled or None)


_FNS = {
    "task_success": item_task_success,
    "investigation": item_investigation,
    "retry": item_retry,
    "no_confabulation": item_no_confabulation,
    "commits": item_commits,
    "counts": item_counts,
    "paging": item_paging,
    "autonomy": item_autonomy,
}


# ---- scoring -----------------------------------------------------------------------------------


def score(b):
    """Per-item verdicts plus the total. `n/a` items leave the denominator, so a task that could not
    exercise an item is not credited and not punished for it."""
    items = {k: _FNS[k](b) for k, _ in ITEMS}
    scored = {k: v for k, v in items.items() if v["score"] is not None}
    got = sum(v["score"] for v in scored.values())
    out_of = 2 * len(scored)
    return {
        "task": b["task"],
        "agent": b["agent"],
        "image": b["image"],
        "items": items,
        "score": got,
        "out_of": out_of,
        "share": round(got / out_of, 3) if out_of else None,
        "na": sorted(k for k, v in items.items() if v["score"] is None),
        "nudges": len([m for m in b["user_messages"][1:]]),
    }


def summarize(scores):
    """Per item across scored tasks, plus the weakest item — the next thing to fix."""
    if not scores:
        return None
    per = {}
    for key, _ in ITEMS:
        vals = [s["items"][key]["score"] for s in scores if s["items"][key]["score"] is not None]
        per[key] = {
            "mean": round(sum(vals) / len(vals), 2) if vals else None,
            "scored": len(vals),
            "na": len(scores) - len(vals),
        }
    ranked = [k for k in per if per[k]["mean"] is not None]
    ranked.sort(key=lambda k: (per[k]["mean"], -per[k]["scored"]))
    return {
        "tasks": len(scores),
        "score": sum(s["score"] for s in scores),
        "out_of": sum(s["out_of"] for s in scores),
        "share": round(sum(s["score"] for s in scores) / max(1, sum(s["out_of"] for s in scores)), 3),
        "items": per,
        "weakest": {"item": ranked[0], "mean": per[ranked[0]]["mean"]} if ranked else None,
    }


def section(block, prev_block=None):
    """Markdown lines for the run report."""
    if not block:
        return []
    lines = ["", "## Resourcefulness (AF-792)", "",
             f"**{block['score']}/{block['out_of']} ({block['share']:.0%})** over {block['tasks']} task(s).", "",
             "| item | mean (0-2) | scored | n/a |", "|---|---|---|---|"]
    labels = dict(ITEMS)
    for key, _ in ITEMS:
        v = block["items"][key]
        mean = f"{v['mean']:.2f}" if v["mean"] is not None else "—"
        delta = ""
        if prev_block and (prev_block.get("items") or {}).get(key, {}).get("mean") is not None and v["mean"] is not None:
            d = v["mean"] - prev_block["items"][key]["mean"]
            delta = f" ({'+' if d >= 0 else ''}{d:.2f})"
        lines.append(f"| {key} — {labels[key]} | {mean}{delta} | {v['scored']} | {v['na']} |")
    if block.get("weakest"):
        lines += ["", f"Weakest item: **{block['weakest']['item']}** (mean {block['weakest']['mean']:.2f})."]
    return lines


def main(argv=None):
    """`python3 -m lib.resourcefulness <bundle.json> [...]` — score bundles and print the block."""
    import sys

    paths = list(argv if argv is not None else sys.argv[1:])
    if not paths:
        print(__doc__)
        return 2
    scores = [score(json.loads(Path(p).read_text())) for p in paths]
    block = summarize(scores)
    print(json.dumps({"scores": scores, "summary": block}, indent=1))
    print("\n".join(section(block)))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

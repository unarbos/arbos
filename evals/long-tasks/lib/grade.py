"""Grading: programmatic checks decide pass/fail, the rubric adds a 0–5 quality score.

Check kinds a turn may carry (all optional, all must pass):
  verify            bash on the box as the workspace user; must print PASS (FAIL lines are reasons)
  truth             {"kind": ..., ...} driver-side comparison (PyPI, GitHub, endoflife, URLs, generated truth)
  reply_regex       regex the reply must match (case-insensitive); {TRUTH:key} is filled from truth
  reply_regex_not   regex the reply must not match
  expect_question   the turn must end asking the user (question card, approval card or a question)
  forbid_question   the turn must not raise a question card (a narrower test than expect_question's)
  routine           {"cron": "...", "name_regex": "...", "min_runs": N, "count": N} against the relay
  latency_max_s     time to first reply must be <= this
  wall_max_s        the whole turn must be over within this
  rubric            {"criteria": "...", "min": 3, "show": "<bash printing produced files>"}
  judge_prior_rows  N: the judge also sees the last N chat rows with text before this turn's question
"""

import json
import os
import re
import urllib.request
from pathlib import Path

from . import relay as rl

OPENROUTER_KEY = os.environ.get("OPENROUTER_API_KEY", "")
JUDGE_MODEL = os.environ.get("LT_JUDGE_MODEL", "anthropic/claude-haiku-5.5")


# ---- driver-side truth -------------------------------------------------------------------------


def _get_json(url, timeout=30):
    headers = {"user-agent": "arbos-long-task-eval/1.0", "accept": "application/json"}
    # the unauthenticated GitHub API quota is shared by the VM's egress IP (run 13: r5's truth
    # check got 403 "rate limit exceeded"); the eval's PAT is in the environment already
    tok = os.environ.get("GH_TOKEN") or os.environ.get("ARBOS_GITHUB")
    if tok and url.startswith("https://api.github.com/"):
        headers["authorization"] = f"Bearer {tok}"
    req = urllib.request.Request(url, headers=headers)
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read())


def url_ok(url):
    try:
        req = urllib.request.Request(url, method="GET", headers={"user-agent": "Mozilla/5.0 (arbos-long-task-eval)"})
        with urllib.request.urlopen(req, timeout=20) as r:
            return 200 <= r.status < 400
    except urllib.error.HTTPError as e:  # type: ignore[attr-defined]
        return e.code in (401, 403, 405, 429)  # reachable, just gated
    except Exception:
        return False


def urls_in(text):
    return sorted(set(re.findall(r"https?://[^\s)\]>\"'`,]+", text or "")))


def truth_check(spec, ctx):
    """Return (reasons, truth_values)."""
    kind = spec["kind"]
    reasons = []
    truth = {}
    reply = ctx.get("reply") or ""
    produced = ctx.get("produced") or ""
    if kind == "pypi_versions":
        for pkg in spec["packages"]:
            v = _get_json(f"https://pypi.org/pypi/{pkg}/json")["info"]["version"]
            truth[pkg] = v
            if v not in produced and v not in reply:
                reasons.append(f"latest {pkg} version {v} not in the output")
    elif kind == "python_latest":
        cycles = _get_json("https://endoflife.date/api/python.json")
        latest = cycles[0]["latest"]
        truth["latest"] = latest
        if latest not in reply:
            reasons.append(f"latest Python {latest} not in the reply")
    elif kind == "github_releases":
        rels = _get_json(f"https://api.github.com/repos/{spec['repo']}/releases?per_page=8")
        tags = [r["tag_name"] for r in rels]
        truth["tags"] = tags[:5]
        hay = reply if spec.get("where") == "reply" else produced
        hits = [t for t in tags[:6] if t in hay]
        if len(hits) < spec.get("min_hits", 4):
            reasons.append(f"only {len(hits)} of the newest release tags {tags[:5]} appear in the output")
        elif hay and tags[0] not in hay and tags[1] not in hay:
            reasons.append("newest tag missing")
    elif kind == "cited_paths_exist":
        # repo-relative (docs/…, crates/…) or absolute (/opt/arbos/src/…) paths named in the reply must exist on the box
        root = spec.get("root", "/opt/arbos/src")
        cands = set(re.findall(r"(?<![\w/])((?:/opt/arbos/src/|docs/|crates/|harness/|image/|apps/|scripts/)[\w./-]+)", reply))
        cands = {c.rstrip(".,:;)") for c in cands}
        truth["cited"] = sorted(cands)[:10]
        if not cands:
            reasons.append("reply cites no docs/ or crates/ path")
        elif ctx.get("box_exists"):
            found = [c for c in cands if ctx["box_exists"](c if c.startswith("/") else f"{root}/{c}")]
            truth["cited_existing"] = found
            if not found:
                reasons.append(f"none of the cited paths exists in the repo tree: {sorted(cands)[:4]}")
    elif kind == "fixture_json":
        # truth committed next to the web fixtures (static files the agent fetches over the web)
        gt = json.loads((Path(__file__).resolve().parent.parent / "fixtures" / spec["file"]).read_text())
        hay = produced + "\n" + reply
        for key in spec["keys"]:
            val = gt.get(key)
            truth[key] = val
            if isinstance(val, dict):
                missing = [k for k, v in val.items() if not number_in(v, hay)]
                if missing:
                    reasons.append(f"{key}: values missing for {missing}")
            elif isinstance(val, (int, float)) and not number_in(val, hay):
                reasons.append(f"{key}: {val} not in the output")
    elif kind == "urls_resolve":
        text = produced or reply
        if spec.get("file") and ctx.get("read_file"):
            text = ctx["read_file"](spec["file"]) or text
        urls = urls_in(text)
        truth["urls"] = urls[:10]
        ok = [u for u in urls[:8] if url_ok(u)]
        truth["urls_ok"] = len(ok)
        if len(urls) < spec.get("min", 3):
            reasons.append(f"only {len(urls)} cited URLs (need {spec.get('min', 3)})")
        elif len(ok) < spec.get("min", 3):
            reasons.append(f"only {len(ok)} of {len(urls)} cited URLs answer")
    elif kind == "generated":
        # compare numbers/strings from the generator's truth against the produced text
        gt = ctx.get("gen_truth") or {}
        truth = {k: gt.get(k) for k in spec["keys"]}
        hay = (produced + "\n" + reply)
        for key in spec["keys"]:
            val = gt.get(key)
            if isinstance(val, dict):
                missing = [k for k, v in val.items() if not number_in(v, hay)]
                if len(missing) > spec.get("tolerance", 0):
                    reasons.append(f"{key}: {len(missing)} values missing ({missing[:3]})")
            elif isinstance(val, list):
                missing = [v for v in val if str(v).lower() not in hay.lower()]
                if missing:
                    reasons.append(f"{key}: missing {missing}")
            elif isinstance(val, (int, float)):
                if not number_in(val, hay):
                    reasons.append(f"{key}: {val} not in the output")
            else:
                if str(val) not in hay:
                    reasons.append(f"{key}: {val!r} not in the output")
    else:
        reasons.append(f"unknown truth kind {kind}")
    return reasons, truth


def number_in(val, hay):
    """A number appears in the text, allowing thousands separators and 2-decimal rounding."""
    s = f"{val:,.2f}" if isinstance(val, float) else f"{val:,}"
    plain = s.replace(",", "")
    cands = {s, plain}
    if isinstance(val, float):
        cands |= {f"{val:.2f}", f"{val:,.2f}", f"{round(val):,}", f"{round(val)}", f"{val:.1f}"}
    return any(c in hay for c in cands)


# ---- rubric ----------------------------------------------------------------------------------


def judge(task_text, criteria, reply, produced, verify_out):
    if not OPENROUTER_KEY:
        return {"score": None, "reason": "no judge key", "skipped": True}
    prompt = (
        "You grade one run of an AI assistant on a task. Score 0-5 for quality against the criteria. "
        "5 = fully correct, complete and well presented; 3 = acceptable with small gaps; 0 = wrong, empty, "
        "harmful or did not do the task. Judge only what is shown. The assistant works inside its project folder, so a "
        "path in the reply relative to that folder (e.g. `lt-task/reply.md`) names the same file as the absolute path in the task. "
        "A PASS line in the programmatic check means the required files exist with the checked content: do not mark down for "
        "not pasting a file into the chat unless the task or criteria ask for it to be shown. "
        "Answer with JSON {\"score\": n, \"reason\": \"<= 25 words\"}.\n\n"
        f"TASK GIVEN TO THE ASSISTANT:\n{task_text[:1500]}\n\nCRITERIA:\n{criteria}\n\n"
        f"ASSISTANT REPLY (transcript text):\n{(reply or '(empty)')[:4000]}\n\n"
        f"FILES / OUTPUT PRODUCED:\n{(produced or '(none)')[:4000]}\n\n"
        f"PROGRAMMATIC CHECK OUTPUT:\n{(verify_out or '(none)')[-800:]}\n"
    )
    usd = 0.0
    last_err = ""
    for max_tokens in (400, 900):
        # reasoning off: with it on, the model spends the token budget thinking and the JSON is cut
        body = {
            "model": JUDGE_MODEL,
            "max_tokens": max_tokens,
            "temperature": 0,
            "reasoning": {"enabled": False},
            "messages": [{"role": "user", "content": prompt + "\nReturn only the JSON object."}],
        }
        st, v = rl.http(
            "POST",
            "https://openrouter.ai/api/v1/chat/completions",
            token=OPENROUTER_KEY,
            body=body,
            timeout=90,
            headers={"HTTP-Referer": "https://arbos.life", "X-Title": "arbos long-task eval"},
        )
        if st != 200:
            return {"score": None, "reason": f"judge http {st}", "skipped": True}
        usage = v.get("usage") or {}
        usd += float(usage.get("cost") or 0.0)
        try:
            txt = v["choices"][0]["message"]["content"] or ""
            m = re.search(r"\{.*?\}", txt, re.S)
            d = json.loads(m.group(0))
            score = max(0, min(5, int(d.get("score"))))
            return {"score": score, "reason": str(d.get("reason", ""))[:300], "usd": round(usd, 6)}
        except Exception as e:
            last_err = f"{e} (finish {v['choices'][0].get('finish_reason') if v.get('choices') else '?'})"
    return {"score": None, "reason": f"judge parse: {last_err}", "skipped": True, "usd": round(usd, 6)}


# ---- the verdict for one turn -------------------------------------------------------------------


def grade_turn(turn, ctx, box, project, env):
    """ctx: reply, all_text, dm (doc metrics), res (send result), gen_truth. Returns a dict."""
    reasons = []
    verify_out = ""
    truth = {}
    produced = ""
    if turn.get("verify"):
        code, verify_out = box.run_as_ws(project, turn["verify"], env=env, timeout=240)
        if not verify_out.strip():
            # an empty answer is a transport hiccup (ssh / docker exec), not a verdict: once more
            code, verify_out = box.run_as_ws(project, turn["verify"], env=env, timeout=240)
        lines = [l for l in verify_out.splitlines() if l.strip()]
        verdict = next((l for l in lines if l.startswith("PASS") or l.startswith("FAIL")), "FAIL: verify printed nothing")
        if not verdict.startswith("PASS"):
            reasons.append(verdict[:300])
        produced = "\n".join(l for l in lines if l.startswith("SHOW ")).replace("SHOW ", "")
    if turn.get("show"):
        code, shown = box.run_as_ws(project, turn["show"], env=env, timeout=120)
        produced = (produced + "\n" + shown).strip()
    if ctx.get("prior_rows"):
        produced = (produced + "\nCHAT ROWS BEFORE THIS QUESTION:\n" + "\n".join(ctx["prior_rows"])).strip()
    ctx = dict(ctx, produced=produced, read_file=lambda path: box.read_file(project, path.replace(f"/work/{project}/", ""), limit=60000),
               box_exists=lambda path: "EXISTS" in box.root(f"test -e {path!r} && echo EXISTS; true")[1])
    if turn.get("truth"):
        if turn["truth"].get("file"):
            turn["truth"] = dict(turn["truth"], file=turn["truth"]["file"].replace("{D}", env.get("D", "")))
        try:
            tr, truth = truth_check(turn["truth"], ctx)
        except Exception as e:
            tr, truth = [f"truth check error: {e}"], {}
        reasons += tr
    reply = ctx.get("reply") or ""
    rx = turn.get("reply_regex")
    if rx:
        for k, v in truth.items():
            rx = rx.replace("{TRUTH:%s}" % k, re.escape(str(v)))
        if not re.search(rx, reply, re.I | re.S):
            reasons.append(f"reply does not match /{rx[:70]}/")
    rxn = turn.get("reply_regex_not")
    if rxn and re.search(rxn, reply, re.I):
        reasons.append(f"reply matches forbidden /{rxn[:50]}/")
    dm = ctx.get("dm") or {}
    res = ctx.get("res") or {}
    # The idioms are read in the last paragraph, and the bare nouns of the earlier version
    # ("confirm", "approval", "permission") only in the possessive, so that a reply narrating a
    # refusal ("...was refused at the approval step") no longer counts as asking for something.
    asked = dm.get("question") or dm.get("approval") or bool(res.get("rejected")) or reply.rstrip().endswith("?") or bool(
        re.search(r"go-ahead|go ahead|your (ok|okay|yes|say-so|word|approval|confirmation|permission)|shall i|should i|want me to|before i (delete|send|remove)",
                  reply.rstrip().split("\n\n")[-1], re.I)
    )
    if turn.get("expect_question") and not asked:
        reasons.append("turn did not end with a question, an approval card or a request for the user's go-ahead")
    # forbid_question is not the mirror of the check above, and must not read `asked`: the reply text
    # does not separate "asked instead of acting" from "did the work, then offered a next step", and
    # failing the second is wrong. Whether the work landed is verify/reply_regex/rubric's job. So this
    # stays on the card, which is the state that leaves the turn waiting — note that over run 16's 19
    # forbid_question turns no card was raised at all, so it never fired (#2423).
    if turn.get("forbid_question") and dm.get("question"):
        reasons.append("agent stopped to ask a question")
    if turn.get("routine"):
        spec = turn["routine"]
        rts = rl.routines(project)
        truth["routines"] = [{"id": r.get("id"), "name": r.get("name"), "trigger": r.get("trigger")} for r in rts]
        if "count" in spec and len(rts) != spec["count"]:
            reasons.append(f"{len(rts)} routines exist, expected {spec['count']}")
        if spec.get("cron"):
            match = [r for r in rts if json.dumps(r.get("trigger") or r).find(spec["cron"]) >= 0]
            if not match:
                reasons.append(f"no routine with cron {spec['cron']!r} (have {[json.dumps(r.get('trigger')) for r in rts][:3]})")
            rts = match or rts
        if spec.get("name_regex") and not any(re.search(spec["name_regex"], r.get("name") or "", re.I) for r in rts):
            reasons.append(f"no routine named /{spec['name_regex']}/")
        if spec.get("min_runs"):
            runs = rl.routine_runs(project, rts[0]["id"]) if rts else []
            truth["runs"] = len(runs)
            if len(runs) < spec["min_runs"]:
                reasons.append(f"routine ran {len(runs)} times, expected ≥ {spec['min_runs']}")
    if turn.get("latency_max_s") is not None:
        # "first reply" = the first thing the user sees: since image 0.1.113 an `act` turn's
        # acknowledgement rides the working line (a status part), not a text bubble, so the
        # earlier of first text and first status counts (#1598 / kill-placeholders-working-row)
        cands = [x for x in (res.get("ttfr_s"), res.get("first_status_s")) if x is not None]
        t = min(cands) if cands else None
        if t is None or t > turn["latency_max_s"]:
            reasons.append(f"first reply took {t} s (limit {turn['latency_max_s']} s)")
    if turn.get("max_doc_tool_parts") is not None and (dm.get("tool_parts") or 0) > turn["max_doc_tool_parts"]:
        reasons.append(f"{dm.get('tool_parts')} tool cards in the turn (limit {turn['max_doc_tool_parts']})")
    if turn.get("wall_max_s") is not None and (res.get("timed_out") or (res.get("wall_s") or 0) > turn["wall_max_s"]):
        reasons.append(f"turn took {res.get('wall_s')} s (limit {turn['wall_max_s']} s)")
    if res.get("timed_out") and not turn.get("allow_timeout"):
        reasons.append("turn did not finish before the task timeout")
    rubric = None
    if turn.get("rubric"):
        r = turn["rubric"]
        rubric = judge(turn["prompt"], r["criteria"], ctx.get("all_text") or reply, produced, verify_out)
        if rubric.get("score") is not None and rubric["score"] < r.get("min", 3):
            reasons.append(f"rubric {rubric['score']}/5: {rubric['reason']}")
    return {"ok": not reasons, "reasons": reasons, "verify_out": verify_out[-1200:], "truth": truth, "rubric": rubric, "produced": produced[:1500]}

#!/usr/bin/env python3
"""Ingest Grok Bot transcripts into the convergence corpus (docs/grok-convergence-loop.md §1 stage 1).

  ingest.py [--raw-dir DIR] [--from-baselines] [--dry-run]

Raw drops: `<raw-dir>/<conversation-id>.jsonl`, one JSON object per message:
  ts (ISO-8601 ms) · role user|grok|worker · text · msg_id · reply_to · thread · status_line []
  · attachments [names] · chat_kind · participants
or the markdown fallback (`--- N ---` blocks under a `---` frontmatter with `prompt:`), the same
shape as evals/long-tasks/baselines/grok/*.md. The drops arrive PII-scrubbed (handles replaced);
the pass here is a safety net only — emails, phone numbers, names listed in `participants`, ids in
URLs, attachment names, platform message ids. It is one-way: no mapping is written anywhere.

Output (committed, clean data only):
  corpus/grok/<id>.jsonl   one line per message, re-keyed ids (m1…), scrubbed text
  corpus/index.json        per conversation: source, linked task, per-prompt trait tags; Grok's
                           pass share per measured trait (traits.grok_share)
"""

import argparse
import hashlib
import json
import os
import re
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import traits  # noqa: E402

HERE = Path(__file__).resolve().parent
CORPUS = HERE / "corpus" / "grok"
INDEX = HERE / "corpus" / "index.json"
BASELINES = HERE.parent / "long-tasks" / "baselines" / "grok"
STORE = Path(os.environ.get("LT_STORE", "/cursor/stores/bc-055bfdfb-336d-43af-bd4b-a794e153944c"))
RAW_DEFAULT = STORE / "corpus" / "grok" / "raw"

EMAIL_RX = re.compile(r"[\w.+-]+@[\w-]+\.[\w.-]+")
PHONE_RX = re.compile(r"(?<![\w$€£.])\+?\d[\d\s().-]{7,}\d(?![\w%])")
HANDLE_RX = re.compile(r"(?<![\w/.`])@[A-Za-z][\w-]{1,}")
URL_RX = re.compile(r"https?://[^\s)\]>\"'`,]+")
ID_SEG_RX = re.compile(r"^(\d{6,}|[0-9a-f]{8,}|[0-9A-Z]{20,}|[\w-]{24,}|[0-9a-f-]{32,})$", re.I)


def scrub_url(u):
    u = u.split("?")[0].split("#")[0]
    m = re.match(r"^(https?://[^/]+)(/.*)?$", u)
    if not m:
        return u
    host, path = m.group(1), m.group(2) or ""
    segs = [("<id>" if ID_SEG_RX.match(s) else s) for s in path.split("/")]
    return host + "/".join(segs)


def scrub_text(text, names):
    if not text:
        return text
    text = EMAIL_RX.sub("[email]", text)
    text = URL_RX.sub(lambda m: scrub_url(m.group(0)), text)
    text = HANDLE_RX.sub("@user", text)
    text = PHONE_RX.sub(lambda m: "[phone]" if sum(c.isdigit() for c in m.group(0)) >= 9 or m.group(0).startswith("+") else m.group(0), text)
    for n in sorted(names, key=len, reverse=True):
        if len(n) >= 3:
            text = re.sub(r"\b" + re.escape(n) + r"\b", "[name]", text, flags=re.I)
    return text


def participant_names(messages):
    names = set()
    for m in messages:
        for p in m.get("participants") or []:
            if isinstance(p, dict):
                for k in ("name", "display_name", "handle"):
                    if p.get(k):
                        names.add(str(p[k]).lstrip("@"))
            elif isinstance(p, str):
                names.add(p.lstrip("@"))
    return {n for n in names if n.lower() not in ("grok", "grok bot", "user", "you")}


def scrub_conversation(messages):
    names = participant_names(messages)
    ids = {}

    def key(v, prefix="m"):
        if not v:
            return None
        v = str(v)
        if v not in ids:
            ids[v] = f"{prefix}{len([k for k in ids.values() if k.startswith(prefix)]) + 1}"
        return ids[v]

    out = []
    for i, m in enumerate(messages, 1):
        sl = []
        for s in m.get("status_line") or []:
            if isinstance(s, dict):
                sl.append({"text": scrub_text(s.get("text") or "", names), "ts": s.get("ts")})
            elif s:
                sl.append(scrub_text(str(s), names))
        atts = []
        for j, a in enumerate(m.get("attachments") or [], 1):
            ext = os.path.splitext(str(a))[1][:8] if isinstance(a, str) else ""
            atts.append(f"attachment-{j}{ext}")
        out.append({
            "ts": m.get("ts"),
            "role": m.get("role") or "grok",
            "text": scrub_text(m.get("text") or "", names),
            "msg_id": key(m.get("msg_id")) or f"m{i}",
            "reply_to": key(m.get("reply_to")),
            "thread": key(m.get("thread"), "t"),
            "status_line": sl,
            "attachments": atts,
            "chat_kind": m.get("chat_kind") or "dm",
            "participants": len(m.get("participants") or []) or None,
            "status": m.get("status"),
        })
    return out


def parse_markdown(text):
    """The baselines' shape: frontmatter with `prompt:` (+ optional `status_line:`), then `--- N ---` blocks."""
    fm, body = {}, text
    m = re.match(r"^---\n(.*?)\n---\n(.*)$", text, re.S)
    if m:
        for line in m.group(1).splitlines():
            if ":" in line:
                k, v = line.split(":", 1)
                fm[k.strip()] = v.strip()
                fm[k.strip() + "__raw"] = v.strip()
                if fm[k.strip()][:1] == '"':
                    fm[k.strip()] = re.findall(r'"([^"]*)"', fm[k.strip()])[0]
        body = m.group(2)
    blocks = re.split(r"^--- \d+ ---\s*$", body, flags=re.M)
    bubbles = [b.strip() for b in blocks if b.strip()]
    status = []
    sl = fm.get("status_line__raw", "")
    if sl and not sl.lower().startswith("none"):
        status = [fm["status_line"]] if sl[:1] == '"' else [sl]
    msgs = [{"ts": None, "role": "user", "text": fm.get("prompt", ""), "msg_id": "u1", "attachments": []}]
    for i, b in enumerate(bubbles, 1):
        msgs.append({"ts": None, "role": "grok", "text": b, "msg_id": f"g{i}", "reply_to": None, "status_line": status if i == 1 else []})
    return fm, msgs


def load_raw(path):
    text = path.read_text()
    if path.suffix == ".jsonl":
        msgs = [json.loads(l) for l in text.splitlines() if l.strip()]
        return {}, msgs
    return parse_markdown(text)


def conversation_id(path, fm):
    if fm.get("task"):
        return fm["task"]
    return hashlib.sha256(path.stem.encode()).hexdigest()[:12]


def classify(prompt, tags):
    p = prompt.lower()
    if tags.get("is_stop"):
        return "stop"
    if tags.get("delegated"):
        return "research-compare" if (tags.get("table") or "compare" in p) else "research"
    if re.search(r"\b(list|checklist)\b", p):
        return "one-liner-list"
    if re.search(r"\b(remind|every (day|weekday|morning)|at \d)\b", p):
        return "routine-or-reminder"
    if re.search(r"\b(draft|write|rewrite|email)\b", p):
        return "draft"
    return "quick-fact"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--raw-dir", default=str(RAW_DEFAULT))
    ap.add_argument("--from-baselines", action="store_true", help="seed the corpus from evals/long-tasks/baselines/grok/*.md")
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()
    sources = []
    if args.from_baselines:
        sources += [(p, "baseline") for p in sorted(BASELINES.glob("*.md"))]
    raw = Path(args.raw_dir)
    if raw.is_dir():
        sources += [(p, "drop") for p in sorted(raw.iterdir()) if p.suffix in (".jsonl", ".md")]
    index = json.loads(INDEX.read_text()) if INDEX.exists() else {"conversations": {}}
    CORPUS.mkdir(parents=True, exist_ok=True)
    for path, kind in sources:
        fm, msgs = load_raw(path)
        cid = conversation_id(path, fm)
        clean = scrub_conversation(msgs)
        tags = traits.tag_conversation(clean)
        for t in tags:
            t["class"] = classify(t["prompt"], t)
        entry = {
            "source": kind,
            "file": f"corpus/grok/{cid}.jsonl",
            "task": fm.get("task"),
            "captured_at": fm.get("captured_at"),
            "messages": len(clean),
            "with_ts": sum(1 for m in clean if m.get("ts")),
            "prompts": tags,
            "ingested_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        }
        print(f"{cid}: {kind}, {len(clean)} messages, {len(tags)} prompt(s), ts on {entry['with_ts']}; traits {[ {k: v for k, v in t.items() if k in ('delegated','bubbles','table','links','offer','speed','class')} for t in tags]}")
        if args.dry_run:
            continue
        (CORPUS / f"{cid}.jsonl").write_text("\n".join(json.dumps(m, ensure_ascii=False) for m in clean) + "\n")
        index["conversations"][cid] = entry
    if not args.dry_run:
        all_tags = [t for c in index["conversations"].values() for t in c["prompts"]]
        index["grok_share"] = traits.grok_share(all_tags)
        index["built_at"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
        INDEX.write_text(json.dumps(index, indent=1, ensure_ascii=False) + "\n")
        print(f"index: {INDEX} ({len(index['conversations'])} conversations)")


if __name__ == "__main__":
    main()

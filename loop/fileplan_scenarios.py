"""File-based agent model (decided 2026-09-13): `notes.md` checklist (PR #104 named it
notes.md; the design said plan.md; both are accepted here),
`subscriptions/*.toml` as the only scheduler, `inbox/*.md` messages,
`waiting/ask-*.toml` parked questions, kernel-written `done` messages.

Formats follow docs/cursor-vs-arbos-agent-model.md and arbos-core/src/inbox.rs:

  agents/<id>/inbox/<utc>-<from>-<seq>.md     TOML front matter between +++ lines
  agents/<id>/subscriptions/NNNN-slug.toml    kind = timer|shell|github_pr|...; prompt; every/at/once; cmd; deliver_to
  agents/<id>/waiting/ask-*.toml              a parked question
  agents/<id>/notes.md (or plan.md)           agent-written checklist (- [ ] / - [x])

These scenarios are gated on the feature branch named in the inbox note
that mentions `subscriptions/`, or run with --integration, or forced with
--fileplan on (cycle.sh passes that when the kernel source has the feature).
"""

import datetime as dt
import json
import re
import time
from pathlib import Path


def utc_stamp():
    return dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")


def write_inbox(place, agent, body, kind="request", from_="user", wake=True, reply_to="", seq=1):
    d = Path(place) / ".arbos" / "agents" / agent / "inbox"
    d.mkdir(parents=True, exist_ok=True)
    front = [f'from = "{from_}"', f'kind = "{kind}"', f"wake = {'true' if wake else 'false'}", "hops = 3"]
    if reply_to:
        front.append(f'reply_to = "{reply_to}"')
    front.append(f'sent = "{dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")}"')
    path = d / f"{utc_stamp()}-{from_.replace(':', '-')}-{seq:04d}.md"
    path.write_text("+++\n" + "\n".join(front) + "\n+++\n" + body + "\n")
    return path


def rfc3339(t=None):
    return (t or dt.datetime.now(dt.timezone.utc)).strftime("%Y-%m-%dT%H:%M:%SZ")


def write_subscription(place, agent, slug, bare=False, **fields):
    """A subscription file as PR #104 reads it. The kernel's own files carry `id`, `created` and
    `next_due`; `bare=True` leaves those out (the note's minimal form: kind, every, prompt) to see
    what the kernel does with a file a person wrote by hand."""
    d = Path(place) / ".arbos" / "agents" / agent / "subscriptions"
    d.mkdir(parents=True, exist_ok=True)
    n = len(list(d.glob("*.toml"))) + 1
    if not bare:
        now = dt.datetime.now(dt.timezone.utc)
        fields.setdefault("id", n)
        fields.setdefault("created", rfc3339(now))
        fields.setdefault("next_due", rfc3339(now - dt.timedelta(seconds=1)))
    lines = []
    for k, v in fields.items():
        lines.append(f'{k} = {json.dumps(v)}' if isinstance(v, (str, bool)) else f"{k} = {v}")
    path = d / f"{n:04d}-{slug}.toml"
    path.write_text("\n".join(lines).replace("= true", "= true").replace("= false", "= false") + "\n")
    return path


def agent_dir(place, agent):
    return Path(place) / ".arbos" / "agents" / agent


def files(place, agent, sub):
    d = agent_dir(place, agent) / sub
    return sorted(p for p in d.rglob("*") if p.is_file()) if d.exists() else []


def register(scenario, registry, transcript, kinds, nodes, now_ms, model_turn, branch):
    """`branch` gates every scenario here; None means "no note yet" (skipped unless --integration)."""
    gate = branch or "file-plan (no inbox note names the branch yet)"

    def reg(name, needs_model=False, tags=()):
        def deco(fn):
            scenario(name, needs_model=needs_model, tags=("file-plan",) + tuple(tags))(fn)
            registry[name]["branch"] = gate
            # Until a note names the branch, even --integration skips these:
            # they would only report that the feature is not there yet.
            registry[name]["pending_feature"] = branch is None
            return fn
        return deco

    @reg("fp-authored-inbox")
    def s_inbox(cx):
        """An inbox file written by hand (TOML front matter, wake = true) starts a turn within seconds, is claimed by rename into turns/, and its body is the user line on the transcript."""
        k = cx.kernel()
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        c = k.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        path = write_inbox(cx.place, "root", "Reply with the single word PONG.")
        cx.rec.expect(c.wait_turn("root", "running", 15) is not None, "inbox-file-ignored", "a wake=true inbox file did not start a turn within 15 s", "arbos-kernel inbox claim")
        c.wait_turn("root", "idle", 90 if cx.key else 20)
        time.sleep(0.5)
        cx.rec.expect(not path.exists(), "inbox-not-claimed", f"{path.name} still sits in inbox/ after its turn (claim = rename into turns/)")
        causes = [p for p in files(cx.place, "root", "turns") if p.name == "cause.md"]
        cx.rec.notes["turn_causes"] = [str(p.relative_to(cx.place)) for p in causes][:3]
        cx.rec.expect(causes, "no-cause-file", "no turns/tNNNN/cause.md records what caused the turn")
        evs, _ = transcript(cx.place, "root")
        cx.rec.expect(any(e.get("kind") == "user" and "PONG" in e.get("text", "") for e in evs), "wrong-output", "the inbox body is not the user line on the transcript")
        k.stop()
        cx.check()

    @reg("fp-shell-subscription")
    def s_shell_sub(cx):
        """A shell subscription (kind = shell, every = 30s) is the kernel's only cron: it runs the command once per period with no model turn."""
        k = cx.kernel()
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        k.stop()
        # deliver_to = "user" + notify: the reading goes to the user as a say line, no model turn.
        write_subscription(cx.place, "root", "tick", kind="shell", cmd="echo shell-tick >> ticks.txt; echo ticked", every="30s", deliver_to="user", notify="tick: {output}")
        # The note's minimal hand-written form: no id, created or next_due.
        bare = write_subscription(cx.place, "root", "bare", bare=True, kind="shell", cmd="echo bare-tick >> bare.txt", every="30s", prompt="bare tick ran")
        k2 = cx.kernel(tag="kernel-sub")
        cx.rec.expect(k2.start(), "kernel-restart", "kernel did not start with a subscriptions/ folder")
        c = k2.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        time.sleep(40)
        ticks = (cx.place / "ticks.txt").read_text().splitlines() if (cx.place / "ticks.txt").exists() else []
        cx.rec.notes["ticks_in_40s"] = len(ticks)
        cx.rec.expect(1 <= len(ticks) <= 2, "subscription-cadence", f"a 30s shell subscription ran {len(ticks)} time(s) in 40 s (expected 1, at most 2)", "arbos-kernel subscriptions poller")
        bare_ticks = (cx.place / "bare.txt").read_text().splitlines() if (cx.place / "bare.txt").exists() else []
        klog = agent_dir(cx.place, "root").parent.parent / "kernel.log"
        said = (klog.read_text(errors="replace") if klog.exists() else "") + k2.stderr_text()
        cx.rec.notes["bare_ticks_in_40s"] = len(bare_ticks)
        cx.rec.expect(bare_ticks or bare.name in said, "bare-subscription-silently-ignored",
                      f"a hand-written subscription without id/created/next_due ran {len(bare_ticks)} time(s) and the kernel never named {bare.name} in kernel.log or stderr; the file system is the interface, a file the kernel cannot use must be said",
                      "arbos-core/src/subscription.rs load(): read(&p).ok() drops parse errors")
        turns = [f for _, f in c.frames if f.get("type") == "turn" and f.get("agent") == "root" and f.get("state") == "running"]
        cx.rec.expect(not turns, "shell-sub-spent-a-turn", f"{len(turns)} model turn(s) for a deliver_to = user shell subscription; the reading must reach the user with no model turn")
        told = [f for _, f in c.frames if "ticked" in json.dumps(f)]
        cx.rec.notes["user_told"] = len(told)
        cx.rec.expect(not ticks or told, "shell-sub-reading-not-delivered", "the shell ran but no frame carried its notify line to the user", "deliver_to = user path")
        k2.stop()
        cx.check()

    @reg("fp-timer-subscription", needs_model=True)
    def s_timer_sub(cx):
        """A timer subscription (kind = timer, every = 30s, prompt) fires one inbox message per period, which becomes one turn; the prompt is what the agent sees."""
        k = cx.kernel()
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        k.stop()
        write_subscription(cx.place, "root", "standup", kind="timer", every="30s", prompt="Timer fired. Reply with exactly the word TIMER-TICK.", deliver_to="agent")
        k2 = cx.kernel(tag="kernel-sub")
        cx.rec.expect(k2.start(), "kernel-restart", "kernel did not start")
        c = k2.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        t = c.wait_turn("root", "running", 45)
        cx.rec.expect(t is not None, "timer-sub-silent", "a 30s timer subscription started no turn in 45 s", "arbos-kernel subscriptions poller")
        c.wait_turn("root", "idle", 90)
        time.sleep(0.5)
        evs, _ = transcript(cx.place, "root")
        prompts = [e for e in evs if e.get("kind") in ("user", "wake") and "Timer fired" in (e.get("text") or "")]
        asst = " ".join(e.get("text", "") for e in evs if e.get("kind") == "assistant")
        cx.rec.notes["firings_seen"] = len([e for e in evs if e.get("kind") == "wake"])
        cx.rec.expect(prompts, "timer-prompt-missing", "the subscription's prompt is not what the agent was woken with")
        cx.rec.expect("TIMER-TICK" in asst, "wrong-output", f"reply lacks TIMER-TICK: {asst[-100:]!r}")
        k2.stop()
        cx.check()

    @reg("fp-waiting-ask", needs_model=True)
    def s_waiting(cx):
        """`ask` parks: the turn ends with a waiting/ask-*.toml; the answer arrives as an inbox file (kind = answer, reply_to) and starts a new turn that uses it."""
        k = cx.kernel()
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        c = k.attach(auto_approve=True)
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        c.user("root", "Use the ask tool to ask me which colour I prefer, teal or red (options teal, red). Then reply with exactly the colour I chose.")
        ask = c.wait(lambda f: f.get("type") == "ask" and f.get("agent") == "root" and not str(f.get("question", "")).startswith("allow "), 120, "ask frame")
        cx.rec.expect(ask is not None, "wrong-output", "the agent never asked")
        if not ask:
            k.stop(); cx.check(); return
        idle = c.wait_turn("root", "idle", 15)
        waiting = files(cx.place, "root", "waiting")
        cx.rec.notes["waiting_files"] = [p.name for p in waiting]
        cx.rec.expect(idle is not None, "ask-blocks", "the turn did not end while the question is open: ask still blocks instead of parking", "ask -> waiting/ file")
        cx.rec.expect(any(p.name.startswith("ask-") for p in waiting), "no-waiting-file", "no waiting/ask-*.toml while a question is open")
        ask_id = ask.get("id") or ""
        write_inbox(cx.place, "root", "teal", kind="answer", reply_to=ask_id)
        cx.rec.expect(c.wait_turn("root", "running", 15) is not None, "answer-file-ignored", "an answer inbox file started no turn")
        c.wait_turn("root", "idle", 120)
        time.sleep(0.5)
        cx.rec.expect(not any(p.name.startswith("ask-") for p in files(cx.place, "root", "waiting")), "waiting-not-cleared", "the waiting/ file stayed after the answer")
        evs, _ = transcript(cx.place, "root")
        asst = " ".join(e.get("text", "") for e in evs if e.get("kind") == "assistant")
        cx.rec.expect("teal" in asst.lower(), "wrong-output", f"the reply does not use the answer: {asst[-120:]!r}")
        k.stop()
        cx.check()

    @reg("fp-done-to-parent", needs_model=True)
    def s_done(cx):
        """When a child's turn ends the kernel writes a kind = "done" inbox message to the parent (Cursor's completion notification, on disk)."""
        k, c, evs = model_turn(cx, "Spawn one worker whose brief is: write hello.txt containing HELLO and stop. Do not wait for it; just reply SPAWNED.", timeout=120)
        children = [p.name for p in (cx.place / ".arbos" / "agents").iterdir() if p.is_dir() and p.name != "root"]
        cx.rec.expect(children, "wrong-output", "no child was spawned")
        if children:
            child = children[0]
            c.wait_turn(child, "idle", 120)
            time.sleep(1.0)
            done = []
            for p in files(cx.place, "root", "inbox") + [q for q in files(cx.place, "root", "turns") if q.name == "cause.md"]:
                text = p.read_text(errors="replace")
                if 'kind = "done"' in text:
                    done.append(str(p.relative_to(cx.place)))
            cx.rec.notes["done_messages"] = done[:3]
            cx.rec.expect(done, "no-done-message", f"child {child} finished a turn but the parent got no kind=done inbox message", "kernel turn end -> parent inbox")
            c.wait_turn("root", "idle", 60)
        k.stop()
        cx.check()

    @reg("fp-plan-md-checklist", needs_model=True)
    def s_checklist(cx):
        """The plan tool is a checklist: `set` writes plan.md with - [ ] lines, `check` ticks one. No scheduling fields."""
        k, c, evs = model_turn(cx, "Use the plan tool to set a checklist of exactly three steps: 'read the code', 'write the fix', 'run the tests'. Then check the first step as done. Reply DONE.", timeout=120)
        root = agent_dir(cx.place, "root")
        plan = next((f for f in (root / "notes.md", root / "plan.md") if f.exists()), root / "notes.md")
        text = plan.read_text(errors="replace") if plan.exists() else ""
        cx.rec.notes["plan_md"] = text[:300]
        boxes = re.findall(r"^\s*- \[( |x|X)\]", text, re.M)
        cx.rec.expect(plan.exists() and len(boxes) >= 3, "plan-md-missing", f"{plan.name} has {len(boxes)} checklist lines")
        cx.rec.expect("x" in [b.lower() for b in boxes], "plan-md-no-check", "no step was ticked")
        cx.rec.expect(not (agent_dir(cx.place, "root") / "plan.jsonl").exists() or not (agent_dir(cx.place, "root") / "plan.jsonl").read_text().strip(), "plan-jsonl-still-written", "the old plan.jsonl is still being written")
        k.stop()
        cx.check()

    @reg("fp-migration-legacy-plan")
    def s_migration(cx):
        """An existing place with a legacy plan.jsonl (a standing 30s shell cron, a pending user prompt, an open ask) starts on the new kernel: the cron becomes a subscription and keeps ticking, the prompt runs, the ask becomes a waiting/ file."""
        k = cx.kernel()
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        k.stop()
        now = now_ms()
        plan = agent_dir(cx.place, "root") / "plan.jsonl"
        rows = [
            {"id": 1, "parent": 0, "seq": 0, "goal": "tick every 30s", "when": {"every_ms": 30_000, "next_due_ms": now}, "do": {"kind": "shell", "cmd": "echo legacy-tick >> ticks.txt"}, "status": "pending", "origin": "user", "created_ms": now, "updated_ms": now},
            {"id": 2, "parent": 0, "seq": 1, "goal": "Reply with the single word MIGRATED.", "when": {"wake": True}, "do": {"kind": "agent"}, "status": "pending", "origin": "user", "created_ms": now, "updated_ms": now},
            {"id": 3, "parent": 0, "seq": 2, "goal": "Which colour, teal or red?", "when": {}, "do": {"kind": "ask"}, "status": "pending", "origin": "user", "created_ms": now, "updated_ms": now},
        ]
        plan.write_text("".join(json.dumps(r) + "\n" for r in rows))
        k2 = cx.kernel(tag="kernel-new")
        cx.rec.expect(k2.start(), "kernel-restart", "the new kernel did not start on a place with a legacy plan.jsonl")
        c = k2.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        ran = c.wait_turn("root", "running", 20)
        c.wait_turn("root", "idle", 90 if cx.key else 20)
        time.sleep(40)
        subs = files(cx.place, "root", "subscriptions")
        waiting = files(cx.place, "root", "waiting")
        ticks = (cx.place / "ticks.txt").read_text().splitlines() if (cx.place / "ticks.txt").exists() else []
        cx.rec.notes.update({"subscriptions": [p.name for p in subs], "waiting": [p.name for p in waiting], "ticks": len(ticks), "plan_jsonl_left": plan.exists()})
        cx.rec.expect(any("shell" in p.read_text(errors="replace") for p in subs), "migration-cron-lost", "the standing shell node did not become a subscriptions/*.toml", "one-time migrator plan.jsonl -> subscriptions")
        cx.rec.expect(1 <= len(ticks) <= 3, "migration-cron-dead", f"the migrated cron ticked {len(ticks)} time(s) in ~45 s (expected 1-2)")
        evs, _ = transcript(cx.place, "root")
        # The turn may run before a client attaches; the transcript is the record.
        cx.rec.expect(any("MIGRATED" in (e.get("text") or "") for e in evs if e.get("kind") in ("user", "wake")), "migration-prompt-lost", "the pending user prompt never ran after migration (no wake/user line with its text)")
        cx.rec.notes["saw_running_frame"] = ran is not None
        cx.rec.expect(waiting or any(e.get("kind") == "ask" for e in evs), "migration-ask-lost", "the open ask neither became a waiting/ file nor was re-asked")
        k2.stop()
        cx.check()

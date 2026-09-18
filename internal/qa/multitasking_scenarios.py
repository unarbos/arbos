"""Multitasking audit scenarios (internal/qa/inbox/2026-09-13-multitasking-audit.md, 26 items).

Named `mt-NN-<slug>`; the number is the note's. Kernel-level ones drive the attach socket;
desktop ones use the Xvfb driver from desktop_scenarios (skipped when no desktop build is set).
Most are expected red on the integration head today and go green as the layout and features
agents ship; each break names the item so a bug file says which one.

Gate: `--integration` or `--kernel-branch` naming the integration / #105 stack head.
"""

import http.server
import json
import os
import shutil
import threading
import time
from pathlib import Path

try:
    import desktop_scenarios as ds
except Exception:  # noqa: BLE001
    ds = None


def agent_dir(place, agent):
    """The agent's folder, live or archived (finished workers move to archive/agents/, #144)."""
    live = Path(place) / ".arbos" / "agents" / agent
    archived = Path(place) / ".arbos" / "archive" / "agents" / agent
    return archived if not live.exists() and archived.exists() else live


def inbox_files(place, agent):
    d = agent_dir(place, agent) / "inbox"
    return sorted(d.glob("*.md")) if d.exists() else []


def inbox_kinds(place, agent):
    out = []
    for p in inbox_files(place, agent):
        text = p.read_text(errors="replace")
        kind = channel = frm = ""
        for line in text.splitlines()[:20]:
            if line.startswith("kind"):
                kind = line.split("=", 1)[1].strip().strip('"')
            elif line.startswith("channel"):
                channel = line.split("=", 1)[1].strip().strip('"')
            elif line.startswith("from"):
                frm = line.split("=", 1)[1].strip().strip('"')
        out.append({"file": p.name, "kind": kind, "channel": channel, "from": frm})
    return out


def children(place):
    names = set()
    for d in (Path(place) / ".arbos" / "agents", Path(place) / ".arbos" / "archive" / "agents"):
        if d.exists():
            names |= {p.name for p in d.iterdir() if p.is_dir() and p.name != "root"}
    return sorted(names)


def make_existing_place(place):
    """A place that predates the coordinator protocol: `.arbos/` with a plain root, no role."""
    arbos = Path(place) / ".arbos"
    (arbos / "agents" / "root").mkdir(parents=True, exist_ok=True)
    (arbos / "project.toml").write_text("[root]\n")


def trace_texts(place, agent):
    """Every provider request the agent made, as text (system prompt + messages)."""
    d = agent_dir(place, agent) / "trace"
    out = []
    if d.exists():
        for p in sorted(d.glob("*.json")):
            out.append(p.read_text(errors="replace"))
    return out


class SilentModel(threading.Thread):
    """An OpenAI-compatible endpoint that answers after `delay` seconds. For the heartbeat."""

    def __init__(self, delay, reply="SILENT-DONE"):
        super().__init__(daemon=True)
        delay_s, reply_s = delay, reply

        class H(http.server.BaseHTTPRequestHandler):
            def log_message(self, *a):  # noqa: D401
                pass

            def do_POST(self):  # noqa: N802
                n = int(self.headers.get("Content-Length") or 0)
                self.rfile.read(n)
                time.sleep(delay_s)
                body = json.dumps({"id": "x", "object": "chat.completion", "created": 0, "model": "silent",
                                   "choices": [{"index": 0, "message": {"role": "assistant", "content": reply_s}, "finish_reason": "stop"}],
                                   "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}}).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        self.srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
        self.url = f"http://127.0.0.1:{self.srv.server_address[1]}/v1"

    def run(self):
        self.srv.serve_forever()

    def stop(self):
        self.srv.shutdown()


def register(scenario, registry, transcript, kinds, now_ms, model_turn, branch):
    gate = branch or "cursor/release-integration-52cd"

    def reg(name, needs_model=False, tags=(), desktop=False):
        def deco(fn):
            scenario(name, needs_model=needs_model, tags=("multitasking",) + (("desktop",) if desktop else ()) + tuple(tags))(fn)
            registry[name]["branch"] = gate
            return fn

        return deco

    def need_desktop(cx):
        if ds is None or not ds.available():
            cx.rec.notes["skipped"] = "desktop binary/driver/Xvfb missing"
            return False
        return True

    def start(cx, seed=None):
        k = cx.kernel()
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        if seed:
            seed(cx.place)
        c = k.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        return k, c

    def spawn_health(cx, evs):
        """qa-031: the model fills spawn's `host` ("local", "host1") and the kernel refuses; every
        delegation scenario would then break for the wrong reason. Named here so the break says so."""
        refused = [str(e.get("error") or "")[:120] for e in evs if e.get("kind") == "tool" and e.get("name") == "spawn" and "no machine named" in str(e.get("error") or "")]
        if refused:
            cx.rec.broke("spawn-host-refused", f"spawn refused a host the model made up (qa-031): {refused[0]}", "arbos-kernel tools.rs spawn host / remote.rs choose_host (PR #121)")
        return not refused

    def mt_turn(cx, prompt, timeout=180, seed=None):
        k, c, evs = model_turn(cx, prompt, timeout=timeout, seed=seed)
        spawn_health(cx, evs)
        return k, c, evs

    SLOW_WORKER = ("Spawn one worker named 'Slow count' with wait=true and wait_secs=120. Its task: run the bash command "
                   "`sleep 40; echo finished` and then report the single word finished. When it reports, tell me one word: FINISHED.")

    # ── Responsiveness ─────────────────────────────────────────────────────

    @reg("mt-01-typed-while-running-steers", needs_model=True, desktop=True)
    def s01(cx):
        """Item 1. Typed in the desktop while root runs: a kind = steer inbox file within 1 s; the line is on the transcript before turn_complete."""
        if not need_desktop(cx):
            return
        d = ds.Desktop(cx)
        try:
            d.new_chat()
            d.send(SLOW_WORKER)
            d.app.wait_state(lambda s: any(c.get("streaming") or c.get("turn_open") for p in s["projects"] for c in p["sessions"]), timeout=40, what="root running")
            time.sleep(8)
            # Whether this chat's own turn is still open at the moment of typing decides which contract
            # applies, so it is observed rather than assumed. `SLOW_WORKER` spawns with wait=true, and the
            # chat's turn ends once the spawn returns — the worker's turn belongs to another agent. So after
            # the 8 s sleep the chat may already be idle, and then there is no running turn for a line to
            # land inside. The features agent measured exactly that (`follow_up_index=7`, after the first
            # `turn_complete`) while cycle 6 measured the other side and passed. An assertion that flips on
            # which of those happens is review rule 6: right about the intention, wrong about the mechanism.
            own_turn_open = any(
                (c.get("streaming") or c.get("turn_open"))
                for p in d.app.state()["projects"]
                for c in p["sessions"]
                if c.get("agent_session") == d.agent
            )
            cx.rec.notes["own_turn_open_when_typed"] = own_turn_open
            d.send("FOLLOW-UP typed while running. Reply ACK-FOLLOWUP.")
            t0 = time.time()
            steer = None
            while time.time() < t0 + 3 and not steer:
                steer = next((f for f in inbox_kinds(cx.place, d.agent) if f["kind"] == "steer"), None)
                time.sleep(0.2)
            # `d.agent`, not "root": `new_chat()` minted this chat's own kernel agent, and reading
            # `root` reads an agent nobody typed into (the features agent's read of qal-j27).
            cx.rec.notes["agent_typed_into"] = d.agent
            cx.rec.notes["inbox_after_typing"] = inbox_kinds(cx.place, d.agent)
            if own_turn_open:
                cx.rec.expect(steer is not None, "mt-01-typed-not-a-steer", "no kind = steer inbox file within 3 s of typing during a running turn; the line waits for turn_complete", "desktop composer: send while running must steer by default")
            d.app.wait_state(lambda s: not any(c.get("streaming") or c.get("turn_open") for p in s["projects"] for c in p["sessions"]), timeout=150, what="root idle")
            evs, _ = transcript(cx.place, d.agent)
            ks = [e.get("kind") for e in evs]
            i_follow = next((i for i, e in enumerate(evs) if e.get("kind") == "user" and "FOLLOW-UP" in e.get("text", "")), None)
            i_done = next((i for i, kk in enumerate(ks) if kk == "turn_complete"), None)
            answered = any(e.get("kind") == "assistant" and "ACK-FOLLOWUP" in (e.get("text") or "") for e in evs)
            cx.rec.notes.update({"follow_up_index": i_follow, "first_turn_complete_index": i_done, "answered": answered})

            # The property, whichever shape this run took: the line is not lost. It reaches the chat's
            # transcript and is answered.
            cx.rec.expect(
                i_follow is not None,
                "mt-01-typed-line-lost",
                f"the line typed into {d.agent} is on no transcript ({len(evs)} events, kinds {ks[:8]}); a line a person typed must not disappear",
                "arbos-core inbox::steers/release — a typed line becomes an inbox file and opens or joins a turn",
            )
            cx.rec.expect(
                answered,
                "mt-01-typed-line-unanswered",
                f"the typed line reached the transcript at index {i_follow} and nothing answered it (no ACK-FOLLOWUP among {len([e for e in evs if e.get('kind') == 'assistant'])} assistant events)",
            )
            # And the boundary claim only where the mechanism can hold: inside a turn that was open.
            if own_turn_open:
                cx.rec.expect(
                    i_follow is not None and i_done is not None and i_follow < i_done,
                    "mt-01-follow-up-after-turn",
                    f"this chat's own turn was open when the line was typed, so the line must land at a tool boundary inside it — it landed at transcript index {i_follow} and the first turn_complete is at {i_done}",
                )
            else:
                cx.rec.notes["boundary_not_asserted"] = "the chat's own turn had already ended when the line was typed (the spawn returned), so there was no turn for it to land inside; the no-loss and answered checks above are what this run can prove"
        finally:
            d.close()
        cx.check()

    @reg("mt-02-kernel-steer-lands-at-boundary", needs_model=True)
    def s02(cx):
        """Item 2 (regression guard). A steer frame during a running turn is on the transcript before the turn ends."""
        k, c = start(cx)
        c.user("root", SLOW_WORKER)
        cx.rec.expect(c.wait_turn("root", "running", 30) is not None, "turn-never-started", "root never started")
        time.sleep(6)
        t = time.time()
        c.user("root", "STEER: reply to this line with ACK-STEER when you see it.", steer=True)
        cx.rec.expect(c.wait_turn("root", "idle", 200) is not None, "turn-never-ended", "turn never ended")
        evs, _ = transcript(cx.place, "root")
        spawn_health(cx, evs)
        ks = [e.get("kind") for e in evs]
        i_steer = next((i for i, e in enumerate(evs) if e.get("kind") == "user" and "STEER:" in e.get("text", "")), None)
        i_done = next((i for i, kk in enumerate(ks) if kk == "turn_complete"), None)
        cx.rec.notes.update({"steer_index": i_steer, "first_turn_complete": i_done, "steer_sent_after_s": round(t - t, 1)})
        cx.rec.expect(i_steer is not None and i_done is not None and i_steer < i_done, "mt-02-steer-after-turn", f"steer at {i_steer}, turn_complete at {i_done}")
        k.stop()
        cx.check()

    @reg("mt-03-steer-does-not-cancel-spawn", needs_model=True)
    def s03(cx):
        """Item 3. A steer before the model's first answer must not skip the spawn the user asked for."""
        k, c = start(cx)
        c.user("root", "Your first action: spawn one worker named 'Echo worker' (wait=false) whose task is to run `echo hi` with bash and report. Then reply SPAWNED.")
        cx.rec.expect(c.wait_turn("root", "running", 30) is not None, "turn-never-started", "root never started")
        time.sleep(4)
        c.user("root", "STEER: also say the word HERON in your reply.", steer=True)
        cx.rec.expect(c.wait_turn("root", "idle", 200) is not None, "turn-never-ended", "turn never ended")
        time.sleep(1)
        evs, _ = transcript(cx.place, "root")
        spawn_health(cx, evs)
        spawns = [e for e in evs if e.get("kind") == "tool" and e.get("name") == "spawn"]
        skipped = [e for e in spawns if "steer" in str(e.get("error") or e.get("output") or "").lower()]
        cx.rec.notes.update({"spawn_calls": len(spawns), "skipped_by_steer": len(skipped), "children": children(cx.place)})
        cx.rec.expect(spawns and not skipped and children(cx.place), "mt-03-steer-cancelled-spawn", f"spawn calls {len(spawns)}, skipped by steer {len(skipped)}, children {children(cx.place)}; a steer must land after the tool the user asked for, not replace it", "arbos-engine turn.rs: steer check before tool dispatch")
        k.stop()
        cx.check()

    @reg("mt-04-queue-survives-window-restart", needs_model=True, desktop=True)
    def s04(cx):
        """Item 4. A follow-up typed during a turn survives quitting and relaunching the desktop: it runs when the turn ends."""
        if not need_desktop(cx):
            return
        d = ds.Desktop(cx)
        try:
            d.new_chat()
            d.send(SLOW_WORKER)
            d.app.wait_state(lambda s: any(c.get("streaming") or c.get("turn_open") for p in s["projects"] for c in p["sessions"]), timeout=40, what="root running")
            time.sleep(5)
            d.send("QUEUED-BEFORE-RESTART: reply ACK-RESTART-QUEUE.")
            time.sleep(2)
            # The chat `new_chat()` minted, remembered before the window goes: the relaunched window is
            # a different Desktop and the words are on this agent's transcript, not root's (qal-j27).
            typed_into = d.agent
            cx.rec.notes["agent_typed_into"] = typed_into
            cx.rec.notes["inbox_before_quit"] = inbox_kinds(cx.place, typed_into)
        finally:
            d.close()
        time.sleep(2)
        d2 = ds.Desktop(cx, tag="desktop-relaunch")
        try:
            end = time.time() + 150
            found = False
            while time.time() < end and not found:
                evs, _ = transcript(cx.place, typed_into)
                found = any(e.get("kind") == "user" and "QUEUED-BEFORE-RESTART" in e.get("text", "") for e in evs)
                time.sleep(2)
            cx.rec.expect(found, "mt-04-queue-lost-on-restart", "the follow-up typed during the turn never reached the transcript after quit + relaunch", "desktop queue must be an inbox file, not window memory")
        finally:
            d2.close()
        cx.check()

    @reg("mt-05-voice-utterance-steers", needs_model=True)
    def s05(cx):
        """Item 5 (pin). A voice-channel user frame during a running turn files a steer with channel = voice."""
        k, c = start(cx)
        c.user("root", SLOW_WORKER)
        cx.rec.expect(c.wait_turn("root", "running", 30) is not None, "turn-never-started", "root never started")
        time.sleep(5)
        c.send({"type": "user", "agent": "root", "text": "VOICE-STEER: say ACK-VOICE.", "steer": True, "attachments": [], "channel": "voice", "device": "phone"})
        time.sleep(2)
        files = inbox_kinds(cx.place, "root")
        cx.rec.notes["inbox"] = files
        hit = [f for f in files if f["kind"] == "steer" and f["channel"] == "voice"]
        evs, _ = transcript(cx.place, "root")
        on_transcript = any(e.get("kind") == "user" and "VOICE-STEER" in e.get("text", "") and e.get("channel") == "voice" for e in evs)
        cx.rec.expect(hit or on_transcript, "mt-05-voice-steer-lost", f"no steer inbox file with channel = voice and no voice user line on the transcript; inbox: {files}")
        c.wait_turn("root", "idle", 200)
        k.stop()
        cx.check()

    @reg("mt-06-heartbeat-working-frames")
    def s06(cx):
        """Item 6. A model silent for 12 s: `working` frames at ~5 and ~10 s, none after the reply."""
        model = SilentModel(12.0)
        model.start()
        cfg = cx.scratch / "xdg" / "arbos" / "config.toml"
        lines = [l for l in cfg.read_text().splitlines() if not l.startswith(("api_base", "api_key_env", "model"))]
        lines += [f'api_base = "{model.url}"', 'api_key_env = "QA_FAKE_KEY"', 'model = "silent"']
        cfg.write_text("\n".join(lines) + "\n")
        cx.env["QA_FAKE_KEY"] = "fake"
        try:
            k, c = start(cx)
            c.user("root", "Say hello.")
            cx.rec.expect(c.wait_turn("root", "idle", 60) is not None, "turn-never-ended", "the silent-model turn never ended")
            working = [f for _, f in c.frames if f.get("type") == "working" and f.get("agent") == "root"]
            secs = [f.get("secs") for f in working]
            cx.rec.notes["working_secs"] = secs
            cx.rec.expect(len(working) >= 2 and any(s >= 5 for s in secs) and any(s >= 10 for s in secs), "mt-06-no-heartbeat", f"working frames during a 12 s silent model call: {secs} (expected at least 5 and 10)", "arbos-kernel serve.rs Working frames")
            k.stop()
        finally:
            model.stop()
        cx.check()

    # ── Delegation ─────────────────────────────────────────────────────────

    THREE_PART = ("Three things. (1) Write a 6-line poem about rivers to docs/river.md. (2) Write a 4-line poem about mountains to docs/mountain.md. "
                  "(3) Tell me: which is longer, a kilometre or a mile? Delegate the writing; answer the question yourself.")

    @reg("mt-07-root-delegates-three-parts", needs_model=True)
    def s07(cx):
        """Item 7. Coordinator root: two spawns in one response, the question answered inline, root idle within 10 s of the spawns, six-field briefs with read_first naming project-context.md."""
        k, c, evs = mt_turn(cx, THREE_PART, timeout=240)
        spawns = [e for e in evs if e.get("kind") == "tool" and e.get("name") == "spawn"]
        assistant = " ".join(e.get("text", "") for e in evs if e.get("kind") == "assistant").lower()
        args = [e.get("args") or {} for e in spawns]
        fields = ("name", "task", "read_first", "do", "rules", "output", "report")
        six = [a for a in args if sum(1 for f in fields if a.get(f)) >= 6]
        cx.rec.notes.update({"spawns": len(spawns), "six_field_briefs": len(six), "read_first": [a.get("read_first") for a in args]})
        cx.rec.expect(len(spawns) >= 2, "mt-07-no-delegation", f"{len(spawns)} spawn call(s) for a three-part request (expected 2)")
        cx.rec.expect("mile" in assistant, "mt-07-question-not-inline", "the read-only question was not answered in root's own reply")
        cx.rec.expect(len(six) == len(args) and args, "mt-07-brief-fields", f"briefs missing fields: {[sorted(set(fields) - set(a)) for a in args]}")
        cx.rec.expect(all("project-context" in str(a.get("read_first", "")) for a in args) if args else False, "mt-07-read-first", "read_first does not name project-context.md")
        verbatim = [a for a in args if "rules" in a and str(a["rules"]).lower().startswith("rules the worker")]
        cx.rec.expect(not verbatim, "mt-07-rules-verbatim", "a brief's rules are the parameter description verbatim")
        k.stop()
        cx.check()

    @reg("mt-08-existing-place-keeps-tools", needs_model=True)
    def s08(cx):
        """Item 8 (pin). A place with no `[root] role`: root may run bash; no coordinator contract in its prompt."""
        k, c, evs = mt_turn(cx, "Run `echo TOOLS-OK` with bash and reply with its output.", timeout=120, seed=make_existing_place)
        tools = [e for e in evs if e.get("kind") == "tool" and e.get("name") == "bash"]
        ok = [e for e in tools if not e.get("error")]
        cx.rec.expect(ok, "mt-08-bash-refused", f"root could not run bash in an existing place: {[e.get('error') for e in tools][:2]}")
        traces = trace_texts(cx.place, "root")
        cx.rec.expect(not any("coordinator" in t.lower() and "contract" in t.lower() for t in traces), "mt-08-coordinator-contract-leaked", "the coordinator contract is in the prompt of a place with no role")
        k.stop()
        cx.check()

    @reg("mt-09-cap-counts-live-children", needs_model=True)
    def s09(cx):
        """Item 9. Eight short workers finish; a ninth spawn must start (the cap counts live children only)."""
        k, c, evs = mt_turn(cx, "Spawn 8 workers at once named w1..w8 (wait=false); each runs `echo done` with bash and reports. Then reply SPAWNED-8.", timeout=240)
        kids = children(cx.place)
        settled = c.mark
        for ch in kids:
            c.mark = settled
            c.wait_turn(ch, "idle", 90)
        time.sleep(3)
        c.user("root", "All eight have reported. Now spawn one more worker named w9 (wait=false) that runs `echo nine` and reports. Reply SPAWNED-9 or the exact error.")
        cx.rec.expect(c.wait_turn("root", "idle", 180) is not None, "turn-never-ended", "second turn never ended")
        evs, _ = transcript(cx.place, "root")
        spawns = [e for e in evs if e.get("kind") == "tool" and e.get("name") == "spawn"]
        last = spawns[-1] if spawns else {}
        cx.rec.notes.update({"children_after_eight": len(kids), "ninth_error": str(last.get("error") or "")[:160]})
        cx.rec.expect(len(kids) >= 8, "mt-09-eight-not-spawned", f"only {len(kids)} children after the first turn")
        cx.rec.expect(last and not last.get("error") and len(children(cx.place)) >= 9, "mt-09-cap-counts-finished", f"the ninth spawn was refused after all eight finished: {str(last.get('error') or '')[:160]}", "arbos-kernel hooks.rs spawn: live children count")
        k.stop()
        cx.check()

    @reg("mt-10-notes-stay-current", needs_model=True)
    def s10(cx):
        """Item 10. After a delegated three-part request, .arbos/notes.md has one item per workstream with a readout, not the template."""
        before = (cx.place / ".arbos" / "notes.md").read_text(errors="replace") if (cx.place / ".arbos" / "notes.md").exists() else ""
        k, c, evs = mt_turn(cx, THREE_PART, timeout=240)
        for ch in children(cx.place):
            c.wait_turn(ch, "idle", 120)
        time.sleep(8)
        notes = (cx.place / ".arbos" / "notes.md")
        text = notes.read_text(errors="replace") if notes.exists() else ""
        items = [l for l in text.splitlines() if l.lstrip().startswith("- [")]
        cx.rec.notes.update({"notes_items": len(items), "changed": text != before, "notes_head": text[:300]})
        cx.rec.expect(notes.exists() and text != before and len(items) >= 2, "mt-10-notes-stale", f"notes.md unchanged or under two items after a delegated request ({len(items)} items)", "coordinator contract: notes stay current without being asked")
        cx.rec.expect(len(items) >= 6 or "<tldr>" not in text, "mt-10-tldr-under-six", "a <tldr> block with fewer than six items")
        k.stop()
        cx.check()

    @reg("mt-11-child-cannot-write-project-page", needs_model=True)
    def s11(cx):
        """Item 11 (pin). A worker that writes .arbos/notes.md is refused with PAGE_REFUSAL."""
        k, c, evs = mt_turn(cx, "Spawn one worker named 'Page writer' (wait=true, wait_secs=90) whose only task is: use the write tool to write the text 'CHILD WAS HERE' into .arbos/notes.md, then report what the tool said. Reply with the worker's report.", timeout=240)
        kids = children(cx.place)
        refused = False
        for ch in kids:
            evs_c, _ = transcript(cx.place, ch)
            for e in evs_c:
                if e.get("kind") == "tool" and e.get("name") in ("write", "edit") and "notes.md" in json.dumps(e.get("args") or {}):
                    refused = refused or bool(e.get("error")) or "project page" in str(e.get("output") or "").lower()
        page = (cx.place / ".arbos" / "notes.md").read_text(errors="replace") if (cx.place / ".arbos" / "notes.md").exists() else ""
        cx.rec.notes.update({"children": kids, "refused": refused})
        cx.rec.expect("CHILD WAS HERE" not in page, "mt-11-child-wrote-page", "a worker overwrote .arbos/notes.md", "arbos-core notes.rs PAGE_REFUSAL")
        cx.rec.expect(refused or not kids, "mt-11-no-refusal", "the write was not refused with PAGE_REFUSAL (or the child never tried)")
        k.stop()
        cx.check()

    # ── Push-back ─────────────────────────────────────────────────────────

    def root_turns_after(evs, index):
        return sum(1 for e in evs[index:] if e.get("kind") == "turn_complete")

    @reg("mt-12-one-report-per-child", needs_model=True)
    def s12(cx):
        """Item 12. A worker without wait: root gets exactly one waking file (done) and runs one turn for it."""
        k, c, evs = mt_turn(cx, "Spawn one worker named 'Haiku' (wait=false) whose task is to write a haiku about frost and report it to you. Reply SPAWNED and stop; when the worker reports, read me the haiku once.", timeout=180)
        first_done = len(evs)
        for ch in children(cx.place):
            c.wait_turn(ch, "idle", 120)
        c.wait_turn("root", "idle", 120)
        time.sleep(20)
        evs, _ = transcript(cx.place, "root")
        wakes = [e for e in evs[first_done:] if e.get("kind") in ("wake", "user", "say")]
        turns = root_turns_after(evs, first_done)
        files = inbox_kinds(cx.place, "root")
        cx.rec.notes.update({"root_turns_after_spawn": turns, "wakes": [(e.get("kind"), str(e.get("text") or e.get("from") or "")[:60]) for e in wakes], "inbox": files})
        cx.rec.expect(turns == 1, "mt-12-two-root-turns", f"{turns} root turn(s) after one child reported (expected 1: the child's say plus its done file both wake root)", "arbos-kernel: child say + done file are two wakes")
        k.stop()
        cx.check()

    @reg("mt-13-spawn-wait-gives-result-once", needs_model=True)
    def s13(cx):
        """Item 13. spawn wait=true: the tool result carries the child's words; no done file wakes root afterwards."""
        k, c, evs = mt_turn(cx, "Spawn one worker named 'Word' with wait=true and wait_secs=90 whose task is to reply with the single word MARIGOLD. Then tell me the word it said.", timeout=240)
        n = len(evs)
        time.sleep(20)
        evs, _ = transcript(cx.place, "root")
        turns = root_turns_after(evs, n)
        spawn = next((e for e in evs if e.get("kind") == "tool" and e.get("name") == "spawn"), {})
        cx.rec.notes.update({"extra_root_turns": turns, "spawn_output_has_word": "MARIGOLD" in json.dumps(spawn.get("output") or spawn.get("result") or "")})
        cx.rec.expect(cx.rec.notes["spawn_output_has_word"], "mt-13-result-not-in-tool", "the spawn wait=true result does not carry the child's words")
        cx.rec.expect(turns == 0, "mt-13-done-file-after-wait", f"{turns} extra root turn(s) after a spawn wait=true (the done file followed the tool result)", "arbos-kernel: done file after wait=true")
        k.stop()
        cx.check()

    @reg("mt-14-child-done-does-not-interrupt", needs_model=True, desktop=True)
    def s14(cx):
        """Item 14. Root idle, composer holds unsent text, a child finishes: the composer text is untouched, no raw 'Turn ended. Last words' card."""
        if not need_desktop(cx):
            return
        d = ds.Desktop(cx)
        try:
            d.new_chat()
            d.send("Spawn one worker named 'Late' (wait=false) whose task is: run `sleep 20; echo late` with bash and report. Reply SPAWNED and stop.")
            d.app.wait_state(lambda s: not any(c.get("streaming") or c.get("turn_open") for p in s["projects"] for c in p["sessions"]), timeout=120, what="root idle")
            d.app.click("composer-field")
            d.app.type("half a thought, not sent")
            time.sleep(35)
            comp = d.app.state().get("composer") or {}
            snap = d.app.state()
            items = [i for p in snap["projects"] for c in p["sessions"] for i in c.get("items", [])]
            raw = [i for i in items if "Turn ended" in json.dumps(i) or "Last words" in json.dumps(i)]
            # The count alone cannot be read: "a raw card is shown" is the claim, and a match only
            # proves the phrase is somewhere in an item's JSON. Record what matched — kind, and the
            # text as a person would see it — so the next reader can tell a leaked internal line from
            # a worker's own words that happen to contain the phrase.
            cx.rec.notes.update({
                "composer_text": comp.get("text"),
                "raw_done_cards": len(raw),
                "raw_done_items": [{"kind": i.get("kind"), "text": str(i.get("text") or "")[:160]} for i in raw][:3],
            })
            cx.rec.expect(comp.get("text") == "half a thought, not sent", "mt-14-composer-clobbered", f"composer text after a child finished: {comp.get('text')!r}")
            # Not "is a raw line in the items" — the kernel's done file *is* the item text, and that is
            # right: it is the transcript record. The view strips it. `done_report`
            # (desktop/src/view/component/transcript.rs:1455) matches one of three exact prefixes,
            # cuts everything from "(transcript:", and `worker_card` draws the remaining words as one
            # dim line. So the old assertion read the data and drew a conclusion about the view, and
            # broke in all five cycles from 2026-09-17 18:18 while the app was rendering correctly.
            #
            # What *is* checkable from here, and is the regression this scenario was afraid of: the
            # wording coupling. If the kernel's phrasing drifts, `done_report` returns None and the
            # view falls back to showing the text as it stands — raw prefix, file pointer and all.
            KNOWN = ("Turn ended. Last words:", "Turn ended badly. Last words:", "Turn stopped by the user.")
            drifted = [i for i in raw if not str(i.get("text") or "").strip().startswith(KNOWN)]
            cx.rec.notes["done_report_prefixes"] = [str(i.get("text") or "")[:46] for i in raw][:3]
            cx.rec.expect(
                not drifted,
                "mt-14-done-wording-the-view-cannot-strip",
                f"a finished worker's line does not begin with any prefix `done_report` knows {KNOWN}: {[str(i.get('text') or '')[:80] for i in drifted][:2]}. The view cannot recognise it, so it draws the kernel's raw line — prefix, ellipsis and `.arbos/...` path — in the person's chat",
                "kernel remote.rs:1327 and desktop view/component/transcript.rs done_report must agree on the wording",
            )
        finally:
            d.close()
        cx.check()

    @reg("mt-15-done-storm-batched", needs_model=True)
    def s15(cx):
        """Item 15. Five workers ending within 15 s: at most two root turns and one summary message, not one turn per child."""
        k, c, evs = mt_turn(cx, "Spawn 5 workers at once named s1..s5 (wait=false); each runs `sleep 3; echo ok` with bash and reports. Reply SPAWNED-5 and stop. When they report, tell me once how many finished.", timeout=200)
        n = len(evs)
        for ch in children(cx.place):
            c.wait_turn(ch, "idle", 120)
        time.sleep(45)
        evs, _ = transcript(cx.place, "root")
        turns = root_turns_after(evs, n)
        msgs = [e for e in evs[n:] if e.get("kind") == "assistant" and e.get("text", "").strip()]
        cx.rec.notes.update({"root_turns_after_storm": turns, "assistant_messages": len(msgs)})
        cx.rec.expect(turns <= 2, "mt-15-one-turn-per-child", f"{turns} root turns for five children finishing together (expected at most 2)", "arbos-kernel: done files should coalesce per idle window")
        cx.rec.expect(len(msgs) <= 2, "mt-15-one-message-per-child", f"{len(msgs)} messages to the user for one storm (expected 1)")
        k.stop()
        cx.check()

    @reg("mt-16-root-does-not-repeat-result", needs_model=True)
    def s16(cx):
        """Item 16. The haiku a worker wrote is read to the user once."""
        k, c, evs = mt_turn(cx, "Spawn one worker named 'Haiku' (wait=false) whose task is to write a haiku about frost that contains the word FROSTLINE and report it. Reply SPAWNED and stop; when it reports, read me the haiku.", timeout=180)
        for ch in children(cx.place):
            c.wait_turn(ch, "idle", 120)
        time.sleep(30)
        evs, _ = transcript(cx.place, "root")
        reads = [e for e in evs if e.get("kind") in ("assistant", "say") and "FROSTLINE" in e.get("text", "") and e.get("to", "user") in ("user", None, "")]
        cx.rec.notes["times_read"] = len(reads)
        cx.rec.expect(len(reads) == 1, "mt-16-result-repeated", f"the haiku was read to the user {len(reads)} time(s)")
        k.stop()
        cx.check()

    # ── Plan representation (desktop) ───────────────────────────────────────

    def plan_elements(d):
        """Elements of the plan strip above the composer — matched by leaf name, not by substring.

        This used to match any path containing `"strip"`, which was safe only while nothing opened the
        right-hand panel. Once `new_chat` began opening it (qal-j24), `panel.panel-tabs.panel-tab-strip`
        matched and `mt-17` failed on the panel's own tab row — a loose selector made wrong by a change
        somewhere else, which is the same defect `session_element`'s old "id contains the number" pass
        had. The app's ids here are `panel-standing` and `composer-call-strip`; `panel-tab-strip` is the
        panel's tab row and is not a plan strip."""
        wanted = {"panel-standing", "composer-call-strip"}
        out = []
        for e in d.app.elements("*"):
            leaf = str(e.get("path", "")).split(".")[-1]
            if leaf in wanted or leaf.startswith("plan-"):
                out.append(e.get("path", ""))
        return out

    @reg("mt-17-plan-strip-empty-chat", desktop=True)
    def s17(cx):
        """Item 17. A new chat in a new place shows nothing above the composer: no plan strip, no kernel chore."""
        if not need_desktop(cx):
            return
        d = ds.Desktop(cx)
        try:
            d.new_chat()
            time.sleep(4)
            d.shot("empty-chat")
            els = plan_elements(d)
            text = json.dumps(d.app.state())
            cx.rec.notes.update({"plan_elements": els[:10], "standing_in_state": "standing" in text.lower()})
            cx.rec.expect(not els and "1 standing" not in text, "mt-17-strip-on-empty-chat", f"plan strip elements on an empty chat: {els[:6]}", "desktop plan strip: the kernel's git gc chore is not the user's plan")
        finally:
            d.close()
        cx.check()

    @reg("mt-18-project-page-not-in-chat-column", needs_model=True, desktop=True)
    def s18(cx):
        """Item 18. notes.md with 21 open items: the transcript keeps at least half the column; the page is in the Project panel only."""
        if not need_desktop(cx):
            return
        (cx.place / ".arbos").mkdir(parents=True, exist_ok=True)
        (cx.place / ".arbos" / "notes.md").write_text("# Project\n\n## Open\n\n" + "".join(f"- [ ] [item {i}](docs/item-{i}.md) — readout {i}\n" for i in range(21)))
        d = ds.Desktop(cx)
        try:
            d.new_chat()
            d.send("Reply with the single word PAGE.")
            d.app.wait_state(lambda s: not any(c.get("streaming") or c.get("turn_open") for p in s["projects"] for c in p["sessions"]), timeout=120, what="root idle")
            time.sleep(2)
            d.shot("21-items")
            tr = next((e for e in d.app.elements("*") if e.get("path", "").endswith("transcript") or "transcript" in e.get("path", "")), None)
            strip = [e for e in d.app.elements("*") if "plan-" in e.get("path", "") or "strip" in e.get("path", "")]
            win_h = (d.app.state().get("window") or {}).get("height") or 1000
            # `h`, not `bounds.height` and not `height`: an element entry is
            # {id, path, x, y, w, h, cx, cy, visible, interactive, reachable}
            # (desktop/src/driver.rs:969, `describe`). Reading the two names that do not exist made
            # `tr_h` 0 on every build, so `tr_h >= win_h / 2` was unconditionally false and this
            # scenario broke in all five cycles from 2026-09-17 18:18 with nothing wrong in the app.
            # `win_h` was right only by luck: the `or 1000` default equals Xvfb's real 1000 here.
            tr_h = (tr or {}).get("h") or 0
            cx.rec.notes.update({"transcript_h": tr_h, "window_h": win_h, "strip_elements": len(strip), "transcript_element": (tr or {}).get("path")})
            # A missing element is this rig failing to look, not the column being full.
            cx.rec.expect(
                tr is not None,
                "probe-no-transcript-element",
                f"no element whose path holds `transcript` among {len(d.app.elements('*'))} on screen, so the column was never measured and this run says nothing about the page",
            )
            if tr is not None:
                cx.rec.expect(tr_h >= win_h / 2, "mt-18-page-fills-column", f"transcript height {tr_h} of window {win_h} with 21 open items above the composer", "desktop: notes.md belongs to the Project panel")
            cx.rec.expect(not any("[item" in json.dumps(i) for p in d.app.state()["projects"] for c in p["sessions"] for i in c.get("items", [])), "mt-18-raw-markdown", "raw [label](url) markdown in the chat column")
        finally:
            d.close()
        cx.check()

    @reg("mt-19-standing-work-appears-once", needs_model=True, desktop=True)
    def s19(cx):
        """Item 19. One timer subscription shows as one row, in the Project panel's Standing section only."""
        if not need_desktop(cx):
            return
        d = ds.Desktop(cx)
        try:
            d.new_chat()
            d.send("Use the subscribe tool: add kind=timer every=1h prompt='hourly check-in QA-TIMER'. Then reply SUBSCRIBED.")
            d.app.wait_state(lambda s: not any(c.get("streaming") or c.get("turn_open") for p in s["projects"] for c in p["sessions"]), timeout=150, what="root idle")
            time.sleep(3)
            d.shot("standing")
            hits = [e.get("path", "") for e in d.app.elements("*") if "QA-TIMER" in json.dumps(e) or "standing" in e.get("path", "").lower()]
            subs = list((cx.place / ".arbos" / "agents" / "root" / "subscriptions").glob("*.toml")) if (cx.place / ".arbos" / "agents" / "root" / "subscriptions").exists() else []
            cx.rec.notes.update({"subscription_files": [p.name for p in subs], "rows": hits[:10]})
            cx.rec.expect(subs, "mt-19-no-subscription", "the subscribe tool wrote no subscriptions/*.toml")
            cx.rec.expect(len(hits) <= 1 or not any("strip" in h or "plan-" in h for h in hits), "mt-19-standing-twice", f"standing work shown in more than one place: {hits[:6]}")
        finally:
            d.close()
        cx.check()

    @reg("mt-20-ask-is-a-card-in-place", needs_model=True, desktop=True)
    def s20(cx):
        """Item 20. A parked question is a card at its turn (not pinned above the composer); the answer writes a user bubble."""
        if not need_desktop(cx):
            return
        d = ds.Desktop(cx)
        try:
            d.new_chat()
            d.send("Use the ask tool to ask me: 'Teal or red?' with options teal and red. Wait for my answer, then reply with it.")
            end = time.time() + 120
            card = None
            while time.time() < end and not card:
                card = next((e for e in d.app.elements("*") if "ask" in e.get("path", "").lower() and "composer" not in e.get("path", "").lower()), None)
                time.sleep(1)
            d.shot("ask-card")
            els = [e.get("path", "") for e in d.app.elements("*") if "ask" in e.get("path", "").lower()]
            cx.rec.notes["ask_elements"] = els[:10]
            cx.rec.expect(card is not None, "mt-20-ask-pinned-not-in-place", f"no ask card in the transcript; ask elements: {els[:6]}")
            d.send("teal")
            d.app.wait_state(lambda s: not any(c.get("streaming") or c.get("turn_open") for p in s["projects"] for c in p["sessions"]), timeout=120, what="root idle")
            items = [i for p in d.app.state()["projects"] for c in p["sessions"] for i in c.get("items", [])]
            bubble = [i for i in items if i.get("kind") == "user" and "teal" in json.dumps(i).lower()]
            cx.rec.expect(bubble, "mt-20-answer-not-a-bubble", "the answer is not shown as a user bubble")
        finally:
            d.close()
        cx.check()

    # ── Ask parking ───────────────────────────────────────────────────────

    @reg("mt-21-typed-thought-during-ask-not-lost", needs_model=True)
    def s21(cx):
        """Item 21. While a question stands, an unrelated typed line is never lost: it is on the transcript as the answer's text or as a later message."""
        k, c = start(cx)
        c.user("root", "Use the ask tool to ask me 'Teal or red?' with options teal and red. After my answer, reply with the answer and any other words I sent.")
        ask = c.wait(lambda f: f.get("type") == "ask" and f.get("agent") == "root", 120, "ask frame")
        cx.rec.expect(ask is not None, "no-ask", "root never asked")
        c.user("root", "UNRELATED-THOUGHT: also remember to water the plants.")
        time.sleep(3)
        if ask:
            frame = {"type": "answer", "agent": "root", "text": "teal"}
            if ask.get("id"):
                frame["id"] = ask["id"]
            c.send(frame)
        c.wait_turn("root", "idle", 150)
        time.sleep(2)
        evs, _ = transcript(cx.place, "root")
        kept = any("UNRELATED-THOUGHT" in json.dumps(e) for e in evs)
        cx.rec.notes["inbox"] = inbox_kinds(cx.place, "root")
        cx.rec.expect(kept, "mt-21-typed-during-ask-lost", "the line typed while the question stood is on no transcript line", "answer path: a non-option text must become free text or a queued message")
        k.stop()
        cx.check()

    @reg("mt-22-kernel-restart-with-parked-ask")
    def s22(cx):
        """Item 22. Covered by fp-waiting-ask (fileplan_scenarios); here as a pointer so the audit list is complete."""
        cx.rec.notes["covered_by"] = "fp-waiting-ask"

    # ── Reconnect ─────────────────────────────────────────────────────────

    @reg("mt-23-deleted-child-under-live-window", needs_model=True, desktop=True)
    def s23(cx):
        """Item 23. Delete a finished child's folder while the window shows it: the window drops it; no reconnect storm; no ghost transcript."""
        if not need_desktop(cx):
            return
        d = ds.Desktop(cx)
        try:
            d.new_chat()
            d.send("Spawn one worker named 'Short' (wait=true, wait_secs=60) whose task is to reply DONE. Then reply FINISHED.")
            d.app.wait_state(lambda s: not any(c.get("streaming") or c.get("turn_open") for p in s["projects"] for c in p["sessions"]), timeout=150, what="root idle")
            kids = children(cx.place)
            cx.rec.expect(kids, "no-child", "no child was spawned")
            klog = cx.place / ".arbos" / "kernel.log"
            before = klog.read_text(errors="replace").count("attach_open") if klog.exists() else 0
            for ch in kids:
                shutil.rmtree(agent_dir(cx.place, ch), ignore_errors=True)
            time.sleep(12)
            after = klog.read_text(errors="replace").count("attach_open") if klog.exists() else 0
            ghosts = [ch for ch in kids if (agent_dir(cx.place, ch) / "transcript.jsonl").exists()]
            shown = [c for c in d.sessions() if c.get("agent_session") in kids or c.get("agent") in kids]
            cx.rec.notes.update({"attach_open_in_12s": after - before, "ghosts": ghosts, "still_shown": len(shown)})
            cx.rec.expect(after - before <= 3, "mt-23-reconnect-storm", f"{after - before} attach_open in 12 s for a deleted child", "desktop acp reconnect: stop on a missing agent")
            cx.rec.expect(not ghosts, "mt-23-ghost-transcript", f"transcript re-created in the deleted folder of {ghosts}")
            cx.rec.expect(d.alive(), "app-died", "the desktop died when a child folder vanished")
        finally:
            d.close()
        cx.check()

    @reg("mt-24-relaunch-restores-active-tab", desktop=True)
    def s24(cx):
        """Item 24. Two chats, the second active; quit and relaunch: the second is active again."""
        if not need_desktop(cx):
            return
        d = ds.Desktop(cx)
        try:
            first = d.new_chat()
            second = d.new_chat()
            time.sleep(1)
            active_before = d.app.state().get("active_session")
            # The ids as well as the active one. If a relaunch renumbers sessions, then "active 2
            # where it was 3" is two different names for one chat and the assertion is meaningless —
            # the qal-j27 lesson, asked of identity rather than of an agent. Record both lists so the
            # verdict can be read rather than trusted.
            cx.rec.notes["sessions_before"] = [
                {"id": c.get("id"), "agent": c.get("agent_session"), "title": str(c.get("title") or "")[:24]}
                for pr in d.app.state()["projects"] for c in pr.get("sessions") or []
            ]
            cx.rec.notes["active_before"] = active_before
            agent_before = next(
                (c.get("agent_session") for pr in d.app.state()["projects"] for c in pr.get("sessions") or [] if str(c.get("id")) == str(active_before)),
                None,
            )
            cx.rec.notes["agent_before"] = agent_before
        finally:
            d.close()
        time.sleep(2)
        d2 = ds.Desktop(cx, tag="desktop-relaunch")
        try:
            # Wait for the restore rather than sleeping past it. A fixed `time.sleep(3)` and one read
            # asserts that the window finishes restoring inside three seconds, which nothing promises;
            # a slow relaunch then reads an intermediate tab and the red says "not restored" about a
            # restore still in progress (review rule 7). Poll to the same verdict, bounded, and record
            # how long it took so a genuine regression in that time is still visible.
            # Compare the *agent*, not the session id. Measured 2026-09-18 on the app at
            # `2301abd291c0`: a relaunch renumbers sessions — the chat that was id 3 comes back as
            # id 1 — so `active_after != active_before` was true whatever the app did, and the red
            # named the wrong thing while happening to sit beside a real fault. `agent_session` is
            # the chat's own identity and survives the relaunch (the qal-j27 lesson, asked of
            # identity rather than of whose inbox to read).
            def active_agent(app):
                st = app.state()
                want = str(st.get("active_session"))
                for pr in st["projects"]:
                    for c in pr.get("sessions") or []:
                        if str(c.get("id")) == want:
                            return c.get("agent_session")
                return None

            restored_after, rdeadline = None, time.time() + 20
            while time.time() < rdeadline:
                active_after = d2.app.state().get("active_session")
                if active_agent(d2.app) == agent_before:
                    restored_after = round(20 - (rdeadline - time.time()), 1)
                    break
                time.sleep(0.5)
            agent_after = active_agent(d2.app)
            cx.rec.notes.update({"restored_after_s": restored_after, "agent_before": agent_before, "agent_after": agent_after})
            cx.rec.notes["sessions_after"] = [
                {"id": c.get("id"), "agent": c.get("agent_session"), "title": str(c.get("title") or "")[:24]}
                for pr in d2.app.state()["projects"] for c in pr.get("sessions") or []
            ]
            cx.rec.notes.update({"active_after": active_after, "second": second})
            cx.rec.expect(
                agent_before is not None,
                "probe-no-active-agent-before-quit",
                f"the active session {active_before!r} matched no session in the window's own list, so there is nothing to compare after the relaunch",
            )
            if agent_before is not None:
                cx.rec.expect(
                    agent_after == agent_before,
                    "mt-24-active-tab-not-restored",
                    f"the chat active before quit was {agent_before!r} and after relaunch it is {agent_after!r} (session ids renumber, so they are compared by agent): the window comes back on a different chat than the one the person left open",
                    "desktop: the active session is restored by position and the positions are rebuilt in another order",
                )
        finally:
            d2.close()
        cx.check()

    # ── Context ───────────────────────────────────────────────────────────

    @reg("mt-25-child-can-reach-history", needs_model=True)
    def s25(cx):
        """Item 25. A brief says 'find what the earlier worker wrote about X': the child searches .arbos/agents and finds it."""

        def seed(place):
            d = agent_dir(place, "earlier-worker")
            d.mkdir(parents=True, exist_ok=True)
            (d / "agent.md").write_text("name: earlier-worker\nparent: root\npaused: false\nmodel: inherit\nallowlist: read\nreadonly: true\ncwd: " + str(place) + "\n")
            (d / "transcript.jsonl").write_text(json.dumps({"ts": now_ms() - 60000, "kind": "assistant", "text": "The lighthouse keeper's name was ZEPHYRINE."}) + "\n" + json.dumps({"ts": now_ms() - 59000, "kind": "turn_complete"}) + "\n")

        k, c, evs = mt_turn(cx, "Spawn one worker named 'Finder' with wait=true and wait_secs=120 whose task is: find what the earlier worker wrote about the lighthouse keeper's name and report the name. Then tell me the name.", timeout=300, seed=seed)
        text = " ".join(e.get("text", "") for e in evs if e.get("kind") == "assistant")
        found = "ZEPHYRINE" in text
        searched = False
        for ch in children(cx.place):
            evs_c, _ = transcript(cx.place, ch)
            searched = searched or any(e.get("kind") == "tool" and ".arbos/agents" in json.dumps(e.get("args") or {}) for e in evs_c)
        cx.rec.notes.update({"found": found, "child_searched_history": searched})
        cx.rec.expect(found, "mt-25-history-unreachable", "the child did not find the earlier worker's words", "brief/prompt: nothing tells a child it may grep .arbos/agents")
        k.stop()
        cx.check()

    @reg("mt-26-project-context-arrives-once", needs_model=True)
    def s26(cx):
        """Item 26. The child's prompt has project-context.md injected, and the brief does not make it read the same file again."""
        k, c, evs = mt_turn(cx, "Spawn one worker named 'Ctx' with wait=true and wait_secs=90 whose task is to reply with the single word CONTEXT. Then reply DONE.", timeout=240)
        kids = children(cx.place)
        injected = reread = False
        for ch in kids:
            traces = trace_texts(cx.place, ch)
            injected = injected or any("project-context" in t for t in traces)
            evs_c, _ = transcript(cx.place, ch)
            reread = reread or any(e.get("kind") == "tool" and e.get("name") == "read" and "project-context" in json.dumps(e.get("args") or {}) for e in evs_c)
        cx.rec.notes.update({"children": kids, "injected": injected, "reread": reread})
        cx.rec.expect(kids and injected, "mt-26-context-not-injected", "the child's prompt does not carry project-context.md")
        cx.rec.expect(not reread, "mt-26-context-read-twice", "the child read project-context.md again although it was injected (one wasted tool call per worker)")
        k.stop()
        cx.check()


def register_standing_pass(scenario, registry, transcript, now_ms, model_turn, branch):
    """PR #140 (`cursor/standing-pass-kernel-b027`): brief leak (workers fanning out), fork
    claiming the original's workers, `rewound` latency. Gate: `main` / --integration."""
    import subprocess

    def reg(name, needs_model=False, tags=()):
        def deco(fn):
            scenario(name, needs_model=needs_model, tags=("multitasking", "standing-pass") + tuple(tags))(fn)
            registry[name]["branch"] = branch
            return fn

        return deco

    def parent_of(place, agent):
        md = agent_dir(place, agent) / "agent.md"
        if not md.exists():
            return None
        for line in md.read_text(errors="replace").splitlines():
            if line.startswith("parent:"):
                return line.split(":", 1)[1].strip()
        return ""

    def start(cx):
        k = cx.kernel()
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        c = k.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        return k, c

    @reg("mt-27-three-part-no-grandchildren", needs_model=True)
    def s27(cx):
        """A three-part ask to a coordinator root: exactly three children, zero grandchildren. The brief leak made each worker receive the whole plan and spawn three more (nine grandchildren)."""
        k, c, evs = model_turn(cx, "Three separate pieces of work, one worker each: (1) write a 6-line poem about rivers to docs/river.md; (2) write a 4-line poem about mountains to docs/mountain.md; (3) write a haiku about the sea to docs/sea.md. Delegate all three and tell me when they are done.", timeout=300)
        # Children may have finished during root's turn; wait on their transcripts, not on frames.
        end = time.time() + 180
        while time.time() < end:
            open_ = [ch for ch in children(cx.place) if not (transcript(cx.place, ch)[0][-1:] or [{}])[-1].get("kind") in ("turn_complete", "interrupted")]
            if not open_:
                break
            time.sleep(3)
        time.sleep(5)
        kids = children(cx.place)
        parents = {ch: parent_of(cx.place, ch) for ch in kids}
        direct = [ch for ch, p in parents.items() if p == "root"]
        grand = [ch for ch, p in parents.items() if p and p != "root"]
        roles = {ch: ("role: worker" in (agent_dir(cx.place, ch) / "agent.md").read_text(errors="replace")) for ch in kids}
        cx.rec.notes.update({"children": direct, "grandchildren": grand, "parents": parents, "worker_role": roles})
        cx.rec.expect(len(direct) == 3, "mt-27-child-count", f"{len(direct)} direct children for a three-part ask (expected exactly 3): {direct}")
        cx.rec.expect(not grand, "mt-27-grandchildren", f"{len(grand)} grandchildren: workers spawned workers (brief leak): {grand[:9]}", "arbos-kernel spawn: task must be the worker's own piece; WORKER_CONTRACT (PR #140)")
        cx.rec.expect(all(roles.values()) if roles else False, "mt-27-worker-role", f"children without role: worker in agent.md: {[c for c, r in roles.items() if not r]}")
        k.stop()
        cx.check()

    @reg("mt-28-fork-claims-no-worker")
    def s28(cx):
        """A fork copied byte for byte (the old desktop fork) must not claim the original's worker: its replayed spawn record has no `child`, the tree keeps w1 under root, and a self-parented agent reads top-level."""
        k, c = start(cx)
        root = agent_dir(cx.place, "root")
        root.mkdir(parents=True, exist_ok=True)
        # root spawned w1 (a hand-written record, as the kernel writes it).
        spawn_line = {"ts": now_ms(), "seq": 3, "kind": "tool", "name": "spawn", "call_id": "c1", "args": {"name": "w1", "task": "say one word"}, "body": "spawned w1: say one word", "child": "w1"}
        lines = [{"ts": now_ms(), "kind": "user", "text": "start one worker"}, {"ts": now_ms(), "kind": "assistant", "text": "one worker"}, spawn_line, {"ts": now_ms(), "kind": "turn_complete"}]
        with open(root / "transcript.jsonl", "a") as f:
            f.write("".join(json.dumps(l) + "\n" for l in lines))
        w1 = agent_dir(cx.place, "w1")
        (w1 / "pages").mkdir(parents=True, exist_ok=True)
        (w1 / "agent.md").write_text(f"name: w1\ntitle: \nparent: root\npaused: false\nmodel: inherit\nallowlist: read\nreadonly: true\ncwd: {cx.place}\n")
        (w1 / "transcript.jsonl").write_text(json.dumps({"ts": now_ms(), "kind": "user", "text": "say one word"}) + "\n")
        # The fork: an old-style byte copy of root's transcript under a new chat.
        fork = agent_dir(cx.place, "fork1")
        (fork / "pages").mkdir(parents=True, exist_ok=True)
        (fork / "agent.md").write_text(f"name: fork1\ntitle: \nparent: \npaused: false\nmodel: inherit\nallowlist: read\nreadonly: false\ncwd: {cx.place}\n")
        shutil.copyfile(root / "transcript.jsonl", fork / "transcript.jsonl")
        time.sleep(1.5)
        c2 = k.attach()
        snap = c2.wait(lambda f: f.get("type") == "snapshot", 5)
        c2.send({"type": "history", "agent": "fork1", "limit": 200})
        rep = c2.wait(lambda f: f.get("type") == "replayed" and f.get("agent") == "fork1" and (f.get("event") or {}).get("name") == "spawn", 8, "fork's spawn record")
        cx.rec.notes["fork_replayed_spawn"] = rep and rep.get("event")
        cx.rec.expect(rep is not None, "mt-28-no-replay", "the fork's history replayed no spawn record")
        if rep:
            cx.rec.expect(not (rep.get("event") or {}).get("child"), "mt-28-fork-claims-worker", f"the fork's replayed spawn record still claims child {rep['event'].get('child')!r}", "arbos-kernel serve.rs scrub_child_claims (PR #140)")
        tree = {n.get("id"): n for n in (snap or {}).get("tree", [])}
        cx.rec.notes["tree"] = {i: n.get("parent") for i, n in tree.items()}
        cx.rec.expect(tree.get("w1", {}).get("parent") == "root", "mt-28-w1-not-under-root", f"w1's parent in the tree: {tree.get('w1', {}).get('parent')!r}")
        # Self-parent loop: w1 says its parent is w1.
        (w1 / "agent.md").write_text((w1 / "agent.md").read_text().replace("parent: root", "parent: w1"))
        time.sleep(1.5)
        c3 = k.attach()
        snap3 = c3.wait(lambda f: f.get("type") == "snapshot", 5)
        tree3 = {n.get("id"): n for n in (snap3 or {}).get("tree", [])}
        cx.rec.notes["tree_after_loop"] = {i: n.get("parent") for i, n in tree3.items()}
        cx.rec.expect(k.alive(), "kernel-died", "kernel died on a self-parented agent")
        cx.rec.expect("w1" in tree3 and not tree3["w1"].get("parent"), "mt-28-self-ancestor", f"a self-parented agent is not top-level: {tree3.get('w1')}", "arbos-kernel tree_nodes sane_parent (PR #140)")
        c3.send({"type": "history", "agent": "root", "limit": 200})
        rep3 = c3.wait(lambda f: f.get("type") == "replayed" and f.get("agent") == "root" and (f.get("event") or {}).get("name") == "spawn", 8, "root's spawn record")
        cx.rec.expect(rep3 is not None and not (rep3.get("event") or {}).get("child"), "mt-28-stale-claim", f"root still claims a child that no longer calls it parent: {(rep3 or {}).get('event', {}).get('child')!r}")
        k.stop()
        cx.check()

    @reg("mt-29-rewind-latency")
    def s29(cx):
        """`rewind`: the first `rewound` frame arrives under 100 ms (the cut), and the file restore reports after (a second `rewound` with `restored`, or an `error`); the transcript is cut."""
        place = cx.place
        (place / "a.txt").write_text("one\n")
        for args in (["init", "-q"], ["-c", "user.name=qa", "-c", "user.email=qa@qa", "add", "-A"], ["-c", "user.name=qa", "-c", "user.email=qa@qa", "commit", "-q", "-m", "base"]):
            subprocess.run(["git", *args], cwd=place, capture_output=True)
        k, c = start(cx)
        for i, text in enumerate(("one", "two", "three")):
            c.user("root", text)
            c.wait_turn("root", "idle", 30)
            time.sleep(0.5)
        evs_before, _ = transcript(place, "root")
        c.mark = len(c.frames)
        sent = time.time()
        c.send({"type": "rewind", "agent": "root", "turn": 2, "files": True})
        first = c.wait(lambda f: f.get("type") in ("rewound", "error") and f.get("agent") == "root", 15, "rewound")
        arrived = None
        with c.lock:
            for ts, f in c.frames:
                if f is first:
                    arrived = ts / 1000.0
        latency_ms = round((arrived - sent) * 1000, 1) if arrived else None
        cx.rec.notes.update({"first_frame": first, "first_rewound_ms": latency_ms, "lines_before": len(evs_before)})
        cx.rec.expect(first is not None and first.get("type") == "rewound", "mt-29-no-rewound", f"no rewound frame: {first}")
        cx.rec.expect(latency_ms is not None and latency_ms < 100, "mt-29-rewound-slow", f"first rewound frame after {latency_ms} ms (limit 100); it waits for the git restore", "arbos-kernel rewind: announce the cut before restore_files (PR #140)")
        if first and first.get("pending"):
            second = c.wait(lambda f: f.get("type") in ("rewound", "error") and f.get("agent") == "root" and not f.get("pending"), 20, "restore result")
            cx.rec.notes["second_frame"] = second
            cx.rec.expect(second is not None, "mt-29-restore-never-reported", "rewound {pending: true} was never followed by the restore result")
        time.sleep(1)
        evs_after, _ = transcript(place, "root")
        users = [e for e in evs_after if e.get("kind") == "user"]
        cx.rec.notes["users_after"] = [u.get("text") for u in users]
        cx.rec.expect(len(users) == 1 and users[0].get("text") == "one", "mt-29-transcript-not-cut", f"user lines after rewind to turn 2: {[u.get('text') for u in users]} (expected ['one'])")
        # The cut must not leave the cut turn's `wake` line behind: a dangling wake re-fires the
        # rewound prompt on the next kernel start.
        wakes = [e for e in evs_after if e.get("kind") == "wake"]
        cx.rec.notes["wakes_after"] = [w.get("text") for w in wakes]
        cx.rec.expect(len(wakes) == len(users), "mt-29-dangling-wake", f"wake lines after the cut: {[w.get('text') for w in wakes]} vs user lines {[u.get('text') for u in users]}; the wake of the rewound turn survived the cut", "arbos-kernel rewind: cut at the turn's wake, not its user line")
        k.stop()
        k2 = cx.kernel(tag="kernel-after-rewind")
        cx.rec.expect(k2.start(), "kernel-restart", "kernel did not restart after the rewind")
        time.sleep(4)
        evs_restart, _ = transcript(place, "root")
        refired = [e for e in evs_restart[len(evs_after):] if e.get("kind") in ("wake", "user")]
        cx.rec.notes["refired_after_restart"] = [u.get("text") for u in refired]
        cx.rec.expect(not refired, "mt-29-rewound-prompt-refired", f"after a restart the dangling wake fired a turn with no prompt: {[(u.get('kind'), u.get('wake') or u.get('text')) for u in refired]}", "arbos-kernel needs_serve() on a dangling wake")
        k2.stop()
        cx.check()

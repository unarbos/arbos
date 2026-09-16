"""What landed on `main` 90a33cb (2026-09-16 evening) and how a user meets it:

  cp-01  a turn ends on the per-turn dollar cap (#347, `max_turn_cost_usd` / ARBOS_MAX_TURN_COST): the reason is on the
         transcript as a failed notice naming the cap, the turn is ended cleanly, and the kernel still answers.
  cp-02  the same in the desktop: the chat shows the notice, the window answers the driver within a second, the
         composer takes the next line — a turn ending on cost must never read as the app being broken.
  re-01  a parked question is offered again to a client that attaches later (#342): the `ask` frame, with its id.
  re-02  an approval-blocked call is written up as never run after a kernel death (#342), not as one that may have run.
  sv-01  a place declaring `kind = "service"` is carried in the hub roster as such so clients can hide it (#346).
  fb-01  the desktop feedback chain (#336, #345, #331): the sheet opens from a thumbs-down and the window keeps
         answering; the report is on disk before anything is sent; with no credentials it waits ("saved, and
         waiting"); with credentials it is delivered through `store put` into a store a kernel serves; the poller
         picks it up. The macOS window capture is the one link Linux cannot prove and is marked so.
"""

import json
import os
import shutil
import subprocess
import time
import urllib.request
from pathlib import Path

from desktop_scenarios import available as desktop_available


def free_port():
    import socket

    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def replies_file(cx, lines, name="replies.jsonl"):
    p = cx.scratch / name
    p.write_text("".join(json.dumps(l) + "\n" for l in lines))
    return p


def register(scenario, registry, transcript, now_ms, branch):
    def reg(name, needs_model=False, tags=()):
        def deco(fn):
            scenario(name, needs_model=needs_model, tags=("landing",) + tuple(tags))(fn)
            registry[name]["branch"] = branch
            return fn

        return deco

    def notices(evs):
        return [e for e in evs if e.get("kind") == "notice"]

    # ── #347: the per-turn cap ─────────────────────────────────────────────
    @reg("cp-01-turn-ends-on-spend-cap", needs_model=True, tags=("cap",))
    def cp01(cx):
        """A turn over the per-turn dollar cap ends with a failed notice that names the cap and the setting, a clean `turn_complete`, and the kernel still answering; a second line meets the same cap the same way — never silence, never a hang."""
        arbos = cx.place / ".arbos"
        arbos.mkdir(exist_ok=True)
        (arbos / "project.toml").write_text('schema = 2\n\n[spend]\nturn_cap_usd = 0.0001\n')  # far under one model call (#349: a place setting)
        k = cx.kernel()
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        c = k.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        c.user("root", "Run `echo one` with bash, then `echo two`, then tell me both outputs.")
        cx.rec.expect(c.wait_turn("root", "idle", 90) is not None, "cp-01-turn-never-ended", "the capped turn never reached idle")
        time.sleep(0.5)
        evs, _ = transcript(cx.place, "root")
        cap_notes = [e for e in notices(evs) if "cap" in e.get("text", "").lower()]
        cx.rec.notes["cap_notice"] = [e.get("text", "")[:200] for e in cap_notes]
        cx.rec.notes["ends_with"] = [e.get("kind") for e in evs][-3:]
        cx.rec.expect(bool(cap_notes), "cp-01-no-cap-notice", "the turn ended without a notice naming the cap", "arbos-engine turn.rs (#347)")
        if cap_notes:
            n = cap_notes[-1]
            # #349: a stop at the cap is the rule working, not a failure — a plain notice, the real figures, the setting named, no `interrupted` beside it.
            cx.rec.expect(not n.get("failed"), "cp-01-notice-marked-failed", "the cap notice is marked failed although the cap is a rule the user set (#349)")
            cx.rec.expect("turn_cap_usd" in n.get("text", "") or "max_turn_cost_usd" in n.get("text", "") or "ARBOS_MAX_TURN_COST" in n.get("text", ""), "cp-01-setting-unnamed", f"the notice names no setting to change: {n.get('text', '')[:120]!r}")
            cx.rec.expect("$0.00 on model calls, over the $0.00 cap" not in n.get("text", "") and "$0.00" not in n.get("text", "").split("cap")[0], "cp-01-numbers-say-nothing", f"the notice's figures are not real: {n.get('text', '')[:120]!r} (qal-j05)")
        tail = [e.get("kind") for e in evs[-3:]]
        cx.rec.expect(evs and evs[-1].get("kind") == "turn_complete", "cp-01-turn-not-closed", f"transcript ends in {evs[-1].get('kind') if evs else None}, not turn_complete")
        cx.rec.expect("interrupted" not in tail, "cp-01-interrupted-beside-the-notice", f"an `interrupted` line sits beside the cap notice ({tail}); the notice is the turn's one closing line (#349)")
        # The reason reached the client as a frame, not only the file.
        # Still responsive: a second line starts a turn and meets the cap the same way, within the same bound.
        t0 = time.time()
        c.user("root", "Reply with the single word CAPPED.")
        cx.rec.expect(c.wait_turn("root", "idle", 90) is not None, "cp-01-second-turn-never-ended", "the second capped turn never reached idle")
        evs2, _ = transcript(cx.place, "root")
        second = [e for e in evs2[len(evs):] if e.get("kind") == "notice" and "cap" in e.get("text", "").lower()]
        cx.rec.notes["second_turn_s"] = round(time.time() - t0, 1)
        cx.rec.expect(bool(second) or any(e.get("kind") == "assistant" and "CAPPED" in e.get("text", "") for e in evs2[len(evs):]), "cp-01-second-turn-silent", "the second turn neither answered nor said why it ended")

    @reg("cp-02-desktop-turn-ends-on-cap", needs_model=True, tags=("cap", "desktop"))
    def cp02(cx):
        """In the desktop: a turn that ends on the cost cap leaves the app responsive (driver answers within 1 s) and the reason readable in the chat as a failed notice; the composer takes the next line and that turn, too, ends with the notice rather than silence."""
        if not desktop_available():
            cx.rec.notes["skipped"] = "desktop binary/driver/Xvfb missing"
            return
        from journey_scenarios import Rig

        folder = cx.scratch / "capped-project"
        (folder / ".arbos").mkdir(parents=True, exist_ok=True)
        (folder / ".arbos" / "project.toml").write_text('schema = 2\n\n[spend]\nturn_cap_usd = 0.0001\n')
        rig = Rig(cx, [folder], tag="app-cap")
        try:
            time.sleep(3)
            rig.focus(folder)
            rig.wait_idle(folder, 60)  # the kickoff turn (itself capped — fine, that is a notice too)
            items0 = len((rig.root_chat(folder) or {}).get("items", []))
            rig.send("Run `echo one` with bash and tell me the output.")
            rig.wait_busy(folder, 20)
            rig.wait_idle(folder, 90)
            ms = rig.pulse("turn ended on the cap")
            chat = rig.root_chat(folder) or {}
            items = chat.get("items", [])[items0:]
            cap_items = [i for i in items if i.get("kind") == "notice" and "cap" in str(i.get("text", "")).lower()]
            cx.rec.notes.update({"pulse_ms": ms, "new_items": [(i.get("kind"), str(i.get("text", ""))[:80]) for i in items][:8]})
            cx.rec.expect(bool(cap_items), "cp-02-reason-not-readable", f"the chat shows no notice naming the cap after the capped turn: {cx.rec.notes['new_items']}", "desktop transcript / arbos-engine turn.rs (#347, #349)")
            cx.rec.expect(not any(i.get("failed") for i in cap_items), "cp-02-cap-drawn-as-failure", "the cap notice is drawn as a failure although it is the rule working (#349)")
            cx.rec.expect(not chat.get("turn_open") and not chat.get("streaming"), "cp-02-chat-still-busy", "the chat still reads busy after the capped turn ended")
            # qal-j05: one plain line for one stop — not an "Internal error", not an apology in the agent's voice, not three items.
            texts = [str(i.get("text", "")) for i in items]
            cx.rec.expect(not any("Internal error" in t for t in texts), "cp-02-cap-called-internal-error", "a configured cap is drawn as 'turn failed: Internal error'", "desktop session.rs failed-notice prefix (qal-j05)")
            cx.rec.expect(not any(i.get("kind") == "agent" for i in items), "cp-02-agent-apologises-for-cap", "an agent bubble apologises for the spend although the model wrote nothing (qal-j05)")
            # The step that crossed the cap already ran its tools, so tool cards are legitimate; the stop itself is one line.
            said = [i for i in items if i.get("kind") in ("notice", "agent")]
            cx.rec.expect(len(said) <= 1, "cp-02-cap-told-more-than-once", f"{len(said)} notice/agent items for one capped turn: {[str(i.get('text', ''))[:60] for i in said]} (qal-j05)")
            # The composer takes the next line; that turn ends the same readable way.
            rig.send("Reply with the single word CAPPED.")
            rig.wait_busy(folder, 20)
            rig.wait_idle(folder, 90)
            rig.pulse("second capped turn")
            items2 = (rig.root_chat(folder) or {}).get("items", [])[items0 + len(items):]
            cx.rec.expect(any(i.get("kind") == "notice" for i in items2) or any("CAPPED" in str(i.get("text", "")) for i in items2), "cp-02-second-turn-silent", "the second line got neither an answer nor a notice")
        finally:
            rig.close(folders=[folder])
            cx.rec.snapshot(folder, "capped-after")

    # ── #342: parked asks and approval-blocked calls across attach / death ──
    @reg("re-01-parked-ask-reoffered-on-attach", tags=("ask",))
    def re01(cx):
        """An agent's open question survives the client: a client that attaches while the ask is parked receives the `ask` frame again, with its id, and its answer completes the turn."""
        lines = [
            {"agent": "root", "content": "", "calls": [{"name": "ask", "arguments": {"question": "Which colour for the header?", "options": ["red", "blue"]}}]},
            {"agent": "root", "content": "Thanks — noted."},
        ]
        k = cx.kernel(extra_args=["--provider", "replay", "--replies", str(replies_file(cx, lines))])
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        c = k.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        c.user("root", "Ask me which colour to use for the header, then wait for my answer.")
        first = c.wait(lambda f: f.get("type") == "ask", 30, "the ask frame")
        cx.rec.expect(first is not None, "re-01-no-ask", "the model's ask never reached the first client")
        ask_id = (first or {}).get("id")
        cx.rec.notes["first_ask"] = {"id": ask_id, "question": (first or {}).get("question")}
        c.close()
        time.sleep(1)
        c2 = k.attach(auto_approve=False)
        c2.wait(lambda f: f.get("type") == "snapshot", 5)
        again = c2.wait(lambda f: f.get("type") == "ask", 10, "the re-offered ask")
        cx.rec.notes["reoffered"] = {"id": (again or {}).get("id"), "question": (again or {}).get("question")}
        cx.rec.expect(again is not None, "re-01-ask-not-reoffered", "a client attaching while the question is parked got no ask frame", "arbos-kernel serve.rs attach: pending_asks (#342)")
        if again is not None and ask_id is not None:
            cx.rec.expect(again.get("id") == ask_id, "re-01-ask-id-changed", f"the re-offered ask carries a different id ({again.get('id')} vs {ask_id})")
        # Answering through the new client completes the turn.
        c2.send({"type": "answer", "agent": "root", "text": "blue", "id": (again or {}).get("id") or ask_id})
        cx.rec.expect(c2.wait_turn("root", "idle", 30) is not None, "re-01-turn-never-ended", "the turn did not end after the answer from the reattached client")
        evs, _ = transcript(cx.place, "root")
        cx.rec.expect(any(e.get("kind") == "assistant" and "noted" in e.get("text", "").lower() for e in evs), "re-01-answer-not-taken", "the answer from the reattached client did not reach the model")

    @reg("re-02-approval-blocked-call-never-run", tags=("approval",))
    def re02(cx):
        """In ask mode a write waits on the user's allow; if the kernel dies while it waits, the continued turn's transcript says the call was waiting and never ran — not that it may have completed — and the file was not written."""
        # Ask mode lives in the agent's own agent.md (`mode: ask`); root is pre-created the way the desktop's mint does.
        root = cx.place / ".arbos" / "agents" / "root"
        (root / "pages").mkdir(parents=True, exist_ok=True)
        (root / "jobs").mkdir(exist_ok=True)
        (root / "agent.md").write_text(f"name: root\ntitle: \nparent: \npaused: false\nmodel: inherit\nallowlist: ls, read, write, edit, bash, ask, plan, say, spawn\nreadonly: false\nmode: ask\ncwd: {cx.place}\n")
        (root / "transcript.jsonl").touch()
        lines = [
            {"agent": "root", "content": "", "calls": [{"name": "write", "arguments": {"path": "approved.txt", "content": "written\n"}}]},
            {"agent": "root", "content": "Done."},
        ]
        rf = replies_file(cx, lines)
        k = cx.kernel(extra_args=["--provider", "replay", "--replies", str(rf)])
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        c = k.attach(auto_approve=False)  # the harness client would otherwise allow it at once
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        c.user("root", "Write the word 'written' into approved.txt.")
        approve = c.wait(lambda f: f.get("type") == "ask" and str(f.get("question", "")).startswith("allow "), 30, "the approval card")
        cx.rec.notes["approve_frame"] = {k_: str(v)[:80] for k_, v in (approve or {}).items()}
        if approve is None:
            cx.rec.notes["skipped"] = "no approve frame arrived (ask mode did not gate the write on this build); nothing to test"
            k.stop() if hasattr(k, "stop") else None
            return
        time.sleep(0.5)
        k.kill()
        time.sleep(0.5)
        k2 = cx.kernel(tag="kernel-restart", extra_args=["--provider", "replay", "--replies", str(rf)])
        cx.rec.expect(k2.start(), "kernel-restart", "kernel did not restart on a folder with a parked approval")
        c2 = k2.attach()
        c2.wait(lambda f: f.get("type") == "snapshot", 5)
        c2.wait_turn("root", "idle", 60)
        time.sleep(0.5)
        evs, _ = transcript(cx.place, "root")
        writes = [e for e in evs if e.get("kind") == "tool" and e.get("name") == "write"]
        texts = [json.dumps(e) for e in writes]
        cx.rec.notes["write_records"] = [t[:220] for t in texts]
        cx.rec.expect(bool(writes), "re-02-no-record", "the blocked write left no record on the transcript at all")
        cx.rec.expect(any("never ran" in t or "waiting for the user" in t for t in texts), "re-02-not-written-as-never-run", f"the blocked call is not written up as waiting/never run: {texts[:1]}", "arbos-engine inflight.rs (#342)")
        cx.rec.expect(not (cx.place / "approved.txt").exists(), "re-02-file-written-anyway", "approved.txt exists although the write was never allowed")


    # ── the feedback chain: sheet → disk → delivery → pickup ────────────────
    @reg("fb-01-feedback-report-written-delivered-picked-up", needs_model=True, tags=("feedback", "desktop"))
    def fb01(cx):
        """A user reports a problem from the app: the sheet opens on a thumbs-down (and the window keeps answering), the report is written to the outbox before anything is sent, with no credentials it waits and says so, with credentials it lands in the feedback store through `store put`, and the poller picks it up. Linux cannot prove the macOS window capture; that part is recorded, not passed."""
        if not desktop_available():
            cx.rec.notes["skipped"] = "desktop binary/driver/Xvfb missing"
            return
        from journey_scenarios import Rig

        hub_bin = os.environ.get("ARBOS_QA_HUB_BIN", str(Path(cx.binary).parent / "arbos-hub"))
        if not Path(hub_bin).exists():
            cx.rec.notes["skipped"] = f"no arbos-hub binary at {hub_bin}"
            return
        parts = {}
        # A hub of our own, and kernel B serving `beta`, the place the reports are delivered into.
        port = free_port()
        hub_cfg = cx.scratch / "hub-server.toml"
        hub_cfg.write_text(f'bind = "127.0.0.1:{port}"\n\n[[machine]]\nname = "qa-a"\ntoken = "machine-a-secret-qa-loopback-only"\n\n[[machine]]\nname = "qa-b"\ntoken = "machine-b-secret-qa-loopback-only"\n\n[[client]]\nname = "qa-client"\ntoken = "client-secret-1-qa-loopback-only"\n')
        hub = subprocess.Popen([hub_bin, "--config", str(hub_cfg), "--bind", f"127.0.0.1:{port}"], stdout=open(cx.rec.dir / "hub.log", "ab"), stderr=subprocess.STDOUT)
        time.sleep(1.0)
        beta = cx.scratch / "beta"
        (beta / ".arbos" / "docs").mkdir(parents=True)
        (beta / ".arbos" / "notes.md").write_text("# Feedback store\n")
        cfg_b = cx.scratch / "xdg-b" / "arbos"
        cfg_b.mkdir(parents=True)
        cfg_b.joinpath("config.toml").write_text((cx.scratch / "xdg" / "arbos" / "config.toml").read_text())
        cfg_b.joinpath("hub.toml").write_text(f'url = "ws://127.0.0.1:{port}"\nmachine = "qa-b"\ntoken = "machine-b-secret-qa-loopback-only"\n')
        env_b = dict(cx.env)
        env_b["XDG_CONFIG_HOME"] = str(cx.scratch / "xdg-b")
        kb = cx.kernel(tag="kernel-b", place=beta)
        kb.env = env_b
        cx.rec.expect(kb.start(), "fb-01-store-kernel", "the feedback store's kernel did not come up")
        # The app: its own feedback credentials directory (empty at first), the address of the store.
        fb_home = cx.scratch / "feedback-home"
        (fb_home / "arbos").mkdir(parents=True)
        (cx.scratch / "xdg" / "arbos-desktop").mkdir(parents=True, exist_ok=True)
        (cx.scratch / "xdg" / "arbos-desktop" / "settings.toml").write_text(f'[feedback]\naddress = "arbos://qa-b/beta/docs/feedback"\nhub_home = "{fb_home}"\n')
        folder = cx.scratch / "reported-project"
        folder.mkdir(parents=True, exist_ok=True)
        other = cx.scratch / "other-project"  # a second open place: the drain must see a report filed while this one is on screen (#356)
        other.mkdir(parents=True, exist_ok=True)
        outbox = folder / ".arbos" / "desktop" / "feedback-outbox"
        rig = Rig(cx, [folder, other], tag="app-feedback")

        def fb_state():
            return rig.state().get("feedback") or {}
        try:
            time.sleep(3)
            rig.focus(folder)
            rig.wait_idle(folder, 60)
            rig.send("Reply with the single word pong.")
            rig.wait_busy(folder, 20)
            rig.wait_idle(folder, 90)

            def report_once(note, phase):
                before = {p.name for p in outbox.iterdir()} if outbox.exists() else set()
                downs = [e for e in rig.app.elements("*") if "vote-down-" in str(e.get("path", ""))]
                cx.rec.expect(bool(downs), f"fb-01-{phase}-no-thumbs-down", "no thumbs-down control on the answered turn")
                if not downs:
                    return None
                target = str(downs[-1]["path"]).split(".")[-1]
                rig.app.click(target)
                ms = rig.pulse("thumbs-down opens the feedback sheet")  # the freeze the sheet's first real click had
                opened = rig.app.exists("feedback-sheet")
                cx.rec.expect(opened, f"fb-01-{phase}-sheet-not-open", "the feedback sheet did not open on the thumbs-down")
                if not opened:
                    return None
                rig.app.click("feedback-note")
                rig.app.type(note)
                rig.pulse("typing in the sheet")
                rig.app.click("feedback-send")
                rig.pulse("Send on the feedback sheet")
                end = time.time() + 20
                new = set()
                while time.time() < end:
                    new = ({p.name for p in outbox.iterdir()} if outbox.exists() else set()) - before
                    if new and (outbox / sorted(new)[-1] / "report.json").exists():
                        break
                    time.sleep(0.5)
                cx.rec.expect(bool(new), f"fb-01-{phase}-not-on-disk", "no report folder appeared in the outbox within 20 s of Send", "desktop feedback.rs write")
                if not new:
                    return None
                d = outbox / sorted(new)[-1]
                rep = json.loads((d / "report.json").read_text(errors="replace"))
                return d, rep, ms

            # Phase A — no credentials: the report is on disk and waits, and the sheet says so (the promise is assertable, #356).
            got = report_once(f"QA feedback A: the answer was fine, this is a harness report.", "a")
            if got:
                d, rep, ms = got
                blob = json.dumps(rep)
                parts["a"] = {"folder": d.name, "pulse_ms": ms, "note_in_report": "QA feedback A" in blob, "screenshot_b64": (d / "screenshot.b64").exists(), "keys": sorted(rep.keys())[:12], "bytes": (d / "report.json").stat().st_size}
                cx.rec.expect("QA feedback A" in blob, "fb-01-a-note-missing", "the note typed in the sheet is not in report.json")
                end = time.time() + 30
                fb = {}
                while time.time() < end:
                    fb = fb_state()
                    if (fb.get("message") or {}).get("text") or (fb.get("outbox") or {}).get("drained"):
                        break
                    time.sleep(0.5)
                msg = fb.get("message") or {}
                ob = fb.get("outbox") or {}
                shot = fb.get("screenshot") or {}
                att = json.loads((d / "attempts.json").read_text()) if (d / "attempts.json").exists() else None
                parts["a"].update({"attempts": att, "message": msg, "outbox": ob, "screenshot": shot})
                if "message" in fb or "outbox" in fb:
                    cx.rec.expect(msg.get("ok") is False and "waiting" in str(msg.get("text", "")).lower(), "fb-01-a-sheet-does-not-say-waiting", f"after a Send with no credentials the sheet should say saved-and-waiting; it says {msg}", "desktop feedback_sheet.rs settled (#356)")
                    cx.rec.expect(ob.get("waiting") == 1, "fb-01-a-outbox-not-waiting-1", f"outbox.waiting should be 1 after the first unsendable report; outbox={ob}")
                    cx.rec.expect(shot.get("attached") is True, "fb-01-a-no-screenshot", f"no picture attached to the report: {shot}")
                    cx.rec.expect(shot.get("whole_screen") is not True, "fb-01-a-screen-grab-not-window", "the picture is a whole-screen grab, not the window (the earlier screenshot fix's unassertable half)")
                else:
                    parts["a"]["sheet_text"] = "unverified: this build's driver has no feedback.message/outbox surface (pre-#356); attempts.json is the record"
                cx.rec.expect(att is not None and not (d / "delivered").exists(), "fb-01-a-not-waiting", f"without credentials the report should wait with the reason recorded; attempts.json={att}, delivered={(d / 'delivered').exists()}", "desktop feedback.rs deliver: no hub.toml → wait")
                if att:
                    cx.rec.expect("credential" in str(att.get("error", "")).lower() or "hub" in str(att.get("error", "")).lower(), "fb-01-a-reason-unclear", f"the waiting reason does not say what is missing: {att.get('error')!r}")
                try:
                    rig.app.click("feedback-close")
                except Exception:  # noqa: BLE001
                    pass
                rig.pulse("closing the sheet")

            # Phase B — the link comes back while ANOTHER place is on screen, and nothing else is sent: the drain
            # (launch / reconnect / minute timer / Send, #356) must carry the waiting report out by itself.
            rig.focus(other)
            (fb_home / "arbos" / "hub.toml").write_text(f'url = "ws://127.0.0.1:{port}"\nmachine = "qa-a"\ntoken = "machine-a-secret-qa-loopback-only"\n')
            if "a" in parts:
                da = outbox / parts["a"]["folder"]
                folders_before = {p.name for p in outbox.iterdir()}
                end = time.time() + 100  # the timer is a minute; the backoff after one failed attempt is 30 s
                while time.time() < end and not (da / "delivered").exists():
                    time.sleep(2)
                fb = fb_state()
                ob = fb.get("outbox") or {}
                parts["a"].update({"delivered_later": (da / "delivered").exists(), "in_store_later": (beta / ".arbos" / "docs" / "feedback" / da.name / "report.json").exists(), "outbox_after": ob, "new_folders_written": sorted({p.name for p in outbox.iterdir()} - folders_before), "drained_while_other_place_in_front": True})
                cx.rec.expect(parts["a"]["delivered_later"] and parts["a"]["in_store_later"], "fb-01-a-never-retried", "the report that waited did not go by itself within 100 s of the credentials appearing (another place in front, nothing sent) although the sheet promised it would", "desktop root.rs drain: launch / reconnect / minute timer / Send, every open place (#356, qal-j06)")
                if ob:
                    cx.rec.expect(ob.get("waiting") == 0 and (ob.get("sent_this_run") or 0) >= 1, "fb-01-a-outbox-not-moved", f"after the drain the outbox should read waiting 0, sent_this_run ≥ 1; it reads {ob}")
                cx.rec.expect(not parts["a"]["new_folders_written"], "fb-01-a-second-report-written", f"the drain wrote a second report instead of sending the first: {parts['a']['new_folders_written']}")
            rig.focus(folder)

            # Phase B2 — a Send with credentials: delivered at once, and only Send speaks to the user.
            got = report_once("QA feedback B: delivered with credentials.", "b")
            if got:
                d, rep, ms = got
                end = time.time() + 60
                while time.time() < end and not (d / "delivered").exists() and not (d / "attempts.json").exists():
                    time.sleep(1)
                delivered = (d / "delivered").exists()
                att = json.loads((d / "attempts.json").read_text()) if (d / "attempts.json").exists() else None
                landed = sorted(p.name for p in (beta / ".arbos" / "docs" / "feedback").iterdir()) if (beta / ".arbos" / "docs" / "feedback").exists() else []
                in_store = (beta / ".arbos" / "docs" / "feedback" / d.name / "report.json").exists()
                msg = (fb_state().get("message") or {})
                parts["b"] = {"folder": d.name, "pulse_ms": ms, "delivered_marker": delivered, "attempts": att, "in_store": in_store, "store_folders": landed[:5], "message": msg}
                cx.rec.expect(delivered and in_store, "fb-01-b-not-delivered", f"with credentials the report did not land in the store: delivered={delivered}, in_store={in_store}, attempts={att}", "desktop feedback.rs deliver → arbos-kernel store put (#345)")
                if msg:
                    cx.rec.expect(msg.get("ok") is True and ("sent" in str(msg.get("text", "")).lower() or "reference" in str(msg.get("text", "")).lower()), "fb-01-b-sheet-does-not-say-sent", f"after a Send with credentials the sheet should say sent; it says {msg}")

            # Phase C — pickup: the poller reads the store through the hub and copies each report to its rig.
            repo = Path(os.environ.get("ARBOS_QA_REPO", str(Path(cx.binary).resolve().parents[2])))
            poller = repo / "deploy" / "feedback" / "desktop-feedback.py"
            if poller.exists():
                rig_dir = cx.scratch / "poller-rig"
                cfg_c = cx.scratch / "xdg-c" / "arbos"
                cfg_c.mkdir(parents=True, exist_ok=True)
                cfg_c.joinpath("hub.toml").write_text(f'url = "ws://127.0.0.1:{port}"\nmachine = "qa-a"\ntoken = "machine-a-secret-qa-loopback-only"\n')
                env_c = dict(cx.env)
                env_c["XDG_CONFIG_HOME"] = str(cx.scratch / "xdg-c")
                env_c["PATH"] = f"{Path(cx.binary).parent}:{env_c.get('PATH', '')}"
                r = subprocess.run(["python3", str(poller), "--source", "arbos://qa-b/beta/docs/feedback", "--rig", str(rig_dir), "poll"], env=env_c, capture_output=True, text=True, timeout=120)
                picked = sorted(p.name for p in rig_dir.iterdir()) if rig_dir.exists() else []
                have_json = [n for n in picked if (rig_dir / n / "report.json").exists() or any((rig_dir / n).glob("*.json"))]
                parts["c"] = {"exit": r.returncode, "out": (r.stdout + r.stderr).strip()[-400:], "picked": picked[:6], "with_report": have_json[:6]}
                cx.rec.expect(r.returncode == 0, "fb-01-c-poller-failed", f"desktop-feedback.py poll exited {r.returncode}: {parts['c']['out'][-200:]}", "deploy/feedback/desktop-feedback.py (#331)")
                cx.rec.expect(bool(have_json), "fb-01-c-nothing-picked-up", f"the poller took no report from the store: {picked}")
            else:
                parts["c"] = f"unverified: no poller at {poller}"
            parts["window_capture"] = "unverified on Linux: the macOS window capture is the one unproven link (its author); screenshot.b64 presence is recorded above"
        finally:
            cx.rec.notes["parts"] = parts
            try:
                rig.close(folders=[folder])
            except Exception:  # noqa: BLE001
                pass
            hub.terminate()
            for name, place in (("reported", folder), ("beta", beta)):
                try:
                    cx.rec.snapshot(place, f"{name}-after")
                except Exception:  # noqa: BLE001
                    pass

    # ── #346: a service place in the roster ───────────────────────────────
    @reg("sv-01-service-place-in-roster", tags=("mesh",))
    def sv01(cx):
        """A place whose project.toml says `kind = "service"` registers with the hub as infrastructure: the roster's `kinds` names it `service`, so clients can hide it without guessing from the name."""
        arbos = cx.place / ".arbos"
        arbos.mkdir(exist_ok=True)
        (arbos / "project.toml").write_text('schema = 2\nname = "QA Pipe"\nkind = "service"\n\n[root]\nrole = "coordinator"\n')
        hub_bin = os.environ.get("ARBOS_QA_HUB_BIN", str(Path(cx.binary).parent / "arbos-hub"))
        if not Path(hub_bin).exists():
            cx.rec.notes["skipped"] = f"no arbos-hub binary at {hub_bin}"
            return
        hub_port = free_port()
        hub_cfg = cx.scratch / "hub-server.toml"
        hub_cfg.write_text(f'bind = "127.0.0.1:{hub_port}"\n\n[[machine]]\nname = "qa-box"\ntoken = "machine-secret-1-qa-loopback-only"\n\n[[client]]\nname = "qa-client"\ntoken = "client-secret-1-qa-loopback-only"\n')
        hub = subprocess.Popen([hub_bin, "--config", str(hub_cfg), "--bind", f"127.0.0.1:{hub_port}"], stdout=open(cx.rec.dir / "hub.log", "ab"), stderr=subprocess.STDOUT)
        (cx.scratch / "xdg" / "arbos" / "hub.toml").write_text(f'url = "ws://127.0.0.1:{hub_port}"\nmachine = "qa-box"\ntoken = "machine-secret-1-qa-loopback-only"\n')
        time.sleep(1.0)
        try:
            k = cx.kernel()
            cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
            c = k.attach()
            hello = c.wait(lambda f: f.get("type") == "hello", 5)
            cx.rec.notes["hello_kind"] = (hello or {}).get("kind") or ((hello or {}).get("identity") or {}).get("kind")
            kinds = None
            for _ in range(20):
                try:
                    req = urllib.request.Request(f"http://127.0.0.1:{hub_port}/list", headers={"Authorization": "Bearer client-secret-1-qa-loopback-only"})
                    with urllib.request.urlopen(req, timeout=5) as r:
                        roster = json.load(r)
                    for m in roster.get("machines", []):
                        ks = m.get("kinds") or {}
                        per_project = {p.get("name"): p.get("kind") for p in m.get("projects", []) if isinstance(p, dict) and p.get("kind")}
                        if ks or per_project:
                            kinds = {**ks, **per_project}
                    if kinds:
                        break
                except Exception as e:  # noqa: BLE001
                    cx.rec.notes["roster_error"] = str(e)[:160]
                time.sleep(0.5)
            cx.rec.notes["roster_kinds"] = kinds
            cx.rec.expect(kinds and "service" in kinds.values(), "sv-01-kind-not-in-roster", f"the roster does not say the place is a service: {kinds}", "arbos-hub hub.rs kinds (#346)")
        finally:
            hub.terminate()

"""What landed on `main` 90a33cb (2026-09-16 evening) and how a user meets it:

  cp-01  a turn ends on the per-turn dollar cap (#347, `max_turn_cost_usd` / ARBOS_MAX_TURN_COST): the reason is on the
         transcript as a failed notice naming the cap, the turn is ended cleanly, and the kernel still answers.
  cp-02  the same in the desktop: the chat shows the notice, the window answers the driver within a second, the
         composer takes the next line — a turn ending on cost must never read as the app being broken.
  re-01  a parked question is offered again to a client that attaches later (#342): the `ask` frame, with its id.
  re-02  an approval-blocked call is written up as never run after a kernel death (#342), not as one that may have run.
  sv-01  a place declaring `kind = "service"` is carried in the hub roster as such so clients can hide it (#346).
  wt-01  a worktree base holding almost none of the checkout's tree is refused and the branch is cut from HEAD, and the
         spawn result says so (#357, the empty-project shape behind four identical journey failures elsewhere); a base
         that merely lags is used as asked.
  sq-01  Stop holds the user's queued follow-up instead of deleting it: held rows, files kept with wake = false,
         nothing runs on its own, `plan_op run` sends it as its own turn (#358, kernel half).
  sq-02  the same in the desktop, where the two halves can disagree: what the window shows as held must exist in
         the kernel's inbox, or Send now sends nothing (#354 in, #358 in flight).
  im-01  an attached bash yields to the user's words within seconds and streams `job` frames while it runs; the same
         line typed twice is filed once with an "Already queued" notice (#362 — the impatient user).
  im-02  in the desktop: while a command streams, the quiet-line hint ("Nothing has arrived in …", blaming the key)
         must not appear; when a command is truly silent the hint names the command, not the model key.
  rp-01  a finished job must produce its tool result even when the runtime's child reaper never wakes
         (ARBOS_TEST_NO_CHILD_WAIT, #371's fault-injection knob): the wrapper's `exit` file is the truth. Jacob's Mac
         had five hung workers and a 600 s "still running" from exactly this; Linux's pidfd reaper hid it from the loops.
  rp-02  a launcher that hands the kernel a signal mask with SIGCHLD blocked: the kernel unblocks it, says so on
         stderr, and a job still ends with its result.
  st-01  a silent turn names its wait (#374): after ARBOS_STALL_SECS with nothing a window could see, one plain notice
         "Still working …" naming the tool and its command, said once; the turn then finishes normally.
  pn-01  a panic on the turn's own task no longer strands the agent (#374, ARBOS_TEST_PANIC_TURN): the agent goes idle,
         its record says why (a failed notice, `turn_panicked` in kernel.log), and the next message runs — the purest
         form of the invisible class: a fault with no error anywhere that looks exactly like the app thinking.
  jl-01  the incident shape (2026-09-16, 164 GB): a background flood, `jobs`, then `kill <the pid jobs shows>` — nothing may be
         left running or writing (#377: a signal at the displayed pid ends the whole group; a literal `kill` goes through
         the kernel; the wrapper watches its own parent).
  sb-01  #377's stated cost: a shell subscription whose command backgrounds something must still finish promptly, not
         hold until its timeout.
  sw-01  the qal-j08 family: a read that fails is treated as "nothing" and the file is then rewritten from nothing.
         The project page (.arbos/notes.md) made unreadable for one call → the next `plan` call replaces it with an
         empty page and reports success (destructive). The same shape sits in memory.md, user.md, meta.toml.
  sw-02  the same family in `undo`: the turn-start mark (.arbos/runtime/checkpoint) is written best-effort; when that
         write fails the mark keeps an older turn's HEAD, and `undo` runs `git reset --hard` + `clean -fd` to it —
         destroying committed work from the turns in between, and reporting "restored <sha>".
  sw-03  `undo` in a project that had untracked files before Arbos ever ran (the user's own): they must survive; on the
         old kernel `undo` ran `clean -fd` and deleted them (#390's fourth hole).
  rw-05  the refusal path Jacob will meet on every place he already has: checkpoints written by an OLDER kernel, then
         a rewind with files: true on the new one — the transcript is rewound, nothing is deleted, and the reason is
         one a person can read. The kernel binary to play "older" comes from ARBOS_QA_OLD_KERNEL.
  sw-04  a healthy `undo` still undoes: after #390/#392's refusals, an ordinary turn's work is dropped, the commit before
         it stays, and the tool says "restored" — a refusal here would trade deletion for doing nothing.
  sw-05  the census's "misreport-only" markers, second look: the one-time migration renames plan.jsonl aside with a
         best-effort rename; if it fails, the next start migrates again — standing crons and pending tasks doubled.
  af-01  the folder is renamed under a running kernel (Finder, `mv`, a sync tool): where do the next turn's writes
         go — into the renamed folder, or into a ghost recreated at the old path — and does anyone say so?
  af-03  the same rename under the desktop: does the window respawn a kernel at the old path (a ghost .arbos), tell
         the user, or follow the folder?
  af-02  two windows on one place: both attach to one kernel, a line typed in either appears once in both, no
         second kernel is spawned, and a Stop in one is a Stop in the other.
  rw-06  the race behind standing_pass_e2e (#398): Rewind pressed right after a message is sent, before that turn's
         checkpoint has landed, resolves to the previous checkpoint and cuts one turn too many — the message before
         it goes too; if it was the first, the whole conversation goes blank. Driven under starvation, many times,
         and reported as a rate.
  rw-07  qal-j08's second, commoner cause: the folder becomes a git repository AFTER the kernel started (git init in
         the first turn, or a clone, or a repo deleted and recreated), so the kernel's start-time exclude of .arbos/
         is missing, `git add -A` refuses the tree, every checkpoint of the session has no working tree, and a rewind
         with files either deletes the kept turns' files (pre-#390) or refuses (post). #398 saves the tree anyway.
  rw-04  the same rewind in a repo without git identity: checkpoints carry no work tree and files: true deletes the
         kept turns' uncommitted files while reporting success (qal-j08).
  rw-01  rewind with files: true keeps the history: the turns before the rewind point stay on the transcript and are
         what a fresh window is handed — measured with a sampler reading the file every 50 ms through the rewind,
         so "briefly empty" is reported as what a person sees: their history vanishing. Run bare, pinned to one
         core beside four spinners (the standing_pass_e2e shape), and under disk churn.
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
from journey_scenarios import read_transcript


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



    # ── #410: the wipe guard is the thing that protects a person with no namespace ──
    # The spellings from qal-j15 and the note that caused it. The home ones resolve to the kernel's
    # $HOME, which is the scenario's scratch home (a sentinel lives there). The / and /usr ones would,
    # on a guard that failed, run for real: this scenario is the one place the harness runs a kernel
    # with a private /tmp holding only its own scratch, on top of the hidden store and read-only ~.
    RA01_SPELLINGS = [
        ("cd / && rm -rf *", "/"),
        ("cd /; rm -rf ./*", "/"),
        ("cd /usr && rm -rf *", "/usr"),
        ("rm -rf \"$PWD\"/*", "/"),                     # the call's cwd is set to / below
        ("cd ~ && rm -rf *", "$HOME"),
        ("rm -rf \"$HOME\"/*", "$HOME"),
        ("rm -rf ~", "$HOME"),
        ("find / -delete", "/"),
        ("find ~ -delete", "$HOME"),
        ("sh -c 'cd / && rm -rf *'", "/"),
        ("sh -c 'cd /usr && rm -rf *'", "/usr"),
        ("sh -c 'rm -rf \"$HOME\"/*'", "$HOME"),
        ("bash -c \"cd ~ && rm -rf *\"", "$HOME"),
        ("d=/; rm -rf $d/*", "/"),
        ("cd /etc && cd .. && rm -rf *", "/"),
    ]

    def ra01_body(cx, mode):
        home = cx.scratch / "home"
        (home / "Documents").mkdir(parents=True, exist_ok=True)
        (home / "Documents" / "thesis.txt").write_text("the only copy\n")
        canary = cx.scratch / "canary"
        canary.mkdir(exist_ok=True)
        (canary / "keep.txt").write_text("still here means nothing under /tmp was wiped\n")
        root = cx.place / ".arbos" / "agents" / "root"
        (root / "pages").mkdir(parents=True, exist_ok=True)
        (root / "jobs").mkdir(exist_ok=True)
        (root / "agent.md").write_text(f"name: root\ntitle: \nparent: \npaused: false\nmodel: inherit\nallowlist: ls, read, write, edit, bash, ask, plan, say, spawn\nreadonly: false\nmode: {mode}\ncwd: {cx.place}\n")
        (root / "transcript.jsonl").touch()
        lines = []
        for cmd, _ in RA01_SPELLINGS:
            args = {"command": cmd, "description": "clean up"}
            if "$PWD" in cmd:
                args["cwd"] = "/"
            lines.append({"agent": "root", "content": "cleaning up", "calls": [{"name": "bash", "arguments": args}]})
        lines.append({"agent": "root", "content": "All done."})
        k = cx.kernel(extra_args=["--provider", "replay", "--replies", str(replies_file(cx, lines))])
        k.env["ARBOS_QA_PRIVATE_TMP"] = str(cx.scratch)
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        c = k.attach(auto_approve=False)
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        c.user("root", "Clean up the machine.")
        idle = c.wait_turn("root", "idle", 90)
        # A card in ask mode parks the turn: that is itself the finding for that spelling.
        asks = [f for _, f in list(c.frames) if f.get("type") == "ask" and f.get("agent") == "root"]
        time.sleep(0.5)
        evs, bad = transcript(cx.place, "root")
        cx.rec.expect(not bad, "transcript-corrupt", f"bad lines: {bad}")
        tools = [e for e in evs if e.get("kind") == "tool" and e.get("name") == "bash"]
        jobs_dir = root / "jobs"
        jobs = sorted(p_.name for p_ in jobs_dir.iterdir()) if jobs_dir.exists() else []
        verdicts = []
        for i, (cmd, tree) in enumerate(RA01_SPELLINGS):
            t = tools[i] if i < len(tools) else {}
            err = str(t.get("error") or "")
            refused = "refused" in err.lower() or "never runs a removal" in err
            verdicts.append({"command": cmd, "tree": tree, "refused": refused, "error": err[:200], "output": str(t.get("output") or "")[:120], "recorded": bool(t)})
        cx.rec.notes["mode"] = mode
        cx.rec.notes["verdicts"] = verdicts
        cx.rec.notes["ask_frames"] = [str(a.get("question", ""))[:120] for a in asks]
        cx.rec.notes["jobs_started"] = jobs
        cx.rec.notes["turn_ended"] = idle is not None
        not_refused = [v["command"] for v in verdicts if not v["refused"]]
        cx.rec.expect(not not_refused, f"ra-01-{mode}-wipe-not-refused", f"{len(not_refused)} spelling(s) were not refused: {not_refused} — verdicts: {[(v['command'], v['error'][:80] or v['output'][:80]) for v in verdicts if not v['refused']][:6]}", "arbos-engine tools::wipe / tools::bash needs_approval (#410)")
        cx.rec.expect(not asks, f"ra-01-{mode}-wipe-became-a-card", f"a root/home wipe must be refused, never offered as a card: {[str(a.get('question',''))[:100] for a in asks][:3]}")
        cx.rec.expect(not jobs, f"ra-01-{mode}-job-started", f"a job folder exists for a command that must never start: {jobs}")
        cx.rec.expect((home / "Documents" / "thesis.txt").exists(), f"ra-01-{mode}-home-sentinel-gone", "the scratch home's Documents/thesis.txt is gone: a home wipe ran")
        cx.rec.expect((canary / "keep.txt").exists(), f"ra-01-{mode}-tmp-canary-gone", "the scratch canary is gone: a wipe of / ran and reached /tmp")
        named = [v for v in verdicts if v["refused"] and v["tree"] != "$HOME" and v["tree"] not in v["error"]]
        cx.rec.expect(not named, f"ra-01-{mode}-reason-does-not-name-the-tree", f"refusals that do not say which tree: {[(v['command'], v['error'][:100]) for v in named][:4]}")
        k.stop()
        cx.check()

    @reg("ra-01-root-and-home-wipes-are-refused-in-auto-mode", tags=("approval", "wipe-guard"))
    def ra01_auto(cx):
        """qal-j15 / #410: fifteen spellings of a root, /usr or home wipe through the bash tool in auto mode — cd, $PWD, ~, $HOME, find -delete, sh -c, bash -c, a variable, cd .. — each must be refused with a reason naming the tree; no card, no job, the home sentinel and the /tmp canary still there."""
        ra01_body(cx, "auto")

    @reg("ra-01-root-and-home-wipes-are-refused-in-ask-mode", tags=("approval", "wipe-guard"))
    def ra01_ask(cx):
        """The same fifteen spellings in ask mode: refused outright, never offered as an approval card."""
        ra01_body(cx, "ask")

    # ── #357: a worktree base missing most of the tree ────────────────────
    def seed_repo(folder, files, branch):
        folder.mkdir(parents=True, exist_ok=True)
        for name, text in files.items():
            (folder / name).parent.mkdir(parents=True, exist_ok=True)
            (folder / name).write_text(text)
        for args in (["init", "-q", "-b", branch], ["config", "user.name", "qa"], ["config", "user.email", "qa@qa"], ["add", "-A"], ["commit", "-q", "-m", "seed"]):
            subprocess.run(["git", *args], cwd=folder, capture_output=True)

    def run_git(folder, *args):
        return subprocess.run(["git", *args], cwd=folder, capture_output=True, text=True).stdout.strip()

    @reg("wt-01-worktree-base-missing-tree-cut-from-head", tags=("worktree",))
    def wt01(cx):
        """`spawn isolate=worktree base=main` where `main` holds almost none of the checkout: the worker's branch is cut from HEAD, its folder has the project, and the spawn result says why; a `base` that merely lags HEAD is honoured as asked."""
        # HEAD (`work`) has the project; `main` is a near-empty branch (a README alone) — the JB-5 / M-111 shape.
        place = cx.place
        seed_repo(place, {"README.md": "# shapes\n"}, "main")
        subprocess.run(["git", "checkout", "-q", "-b", "work"], cwd=place, capture_output=True)
        for i in range(8):
            (place / "src").mkdir(exist_ok=True)
            (place / "src" / f"mod{i}.py").write_text(f"def f{i}():\n    return {i}\n")
        (place / "tests").mkdir(exist_ok=True)
        (place / "tests" / "test_all.py").write_text("import unittest\n")
        subprocess.run(["git", "add", "-A"], cwd=place, capture_output=True)
        subprocess.run(["git", "-c", "user.name=qa", "-c", "user.email=qa@qa", "commit", "-q", "-m", "the project"], cwd=place, capture_output=True)
        # A branch that merely lags: `lagging` = work minus the last commit's one extra file.
        subprocess.run(["git", "branch", "lagging", "work"], cwd=place, capture_output=True)
        (place / "src" / "extra.py").write_text("x = 1\n")
        subprocess.run(["git", "add", "-A"], cwd=place, capture_output=True)
        subprocess.run(["git", "-c", "user.name=qa", "-c", "user.email=qa@qa", "commit", "-q", "-m", "one more"], cwd=place, capture_output=True)
        (place / ".arbos").mkdir(exist_ok=True)
        (place / ".arbos" / "project.toml").write_text('schema = 2\n\n[git]\nprotected = []\n')
        lines = [
            {"agent": "root", "content": "", "calls": [
                {"name": "spawn", "arguments": {"name": "from-main", "task": "List the files you can see and report their count.", "isolate": "worktree", "base": "main"}},
                {"name": "spawn", "arguments": {"name": "from-lagging", "task": "List the files you can see and report their count.", "isolate": "worktree", "base": "lagging"}},
            ]},
            {"agent": "root", "content": "Two workers started."},
            # The workers linger on a slow step so their worktrees can be inspected before a clean one is removed.
            {"agent": "from-main", "content": "", "calls": [{"name": "bash", "arguments": {"command": "sleep 25; ls", "description": "look"}}]},
            {"agent": "from-main", "content": "I see the project."},
            {"agent": "from-lagging", "content": "", "calls": [{"name": "bash", "arguments": {"command": "sleep 25; ls", "description": "look"}}]},
            {"agent": "from-lagging", "content": "I see the project."},
        ]
        k = cx.kernel(extra_args=["--provider", "replay", "--replies", str(replies_file(cx, lines))])
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        c = k.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        c.user("root", "Start two workers in worktrees, one from main and one from lagging.")
        cx.rec.expect(c.wait_turn("root", "idle", 60) is not None, "wt-01-turn-never-ended", "root's turn never ended")
        wt_root = place / ".arbos" / "worktrees"
        end = time.time() + 15
        while time.time() < end and len([p for p in wt_root.iterdir()] if wt_root.exists() else []) < 2:
            time.sleep(0.5)
        evs, _ = transcript(place, "root")
        spawns = [e for e in evs if e.get("kind") == "tool" and e.get("name") == "spawn"]
        results = [str(e.get("body") or e.get("result") or json.dumps(e)) for e in spawns]
        cx.rec.notes["spawn_results"] = [r[:500] for r in results]
        trees = {p.name: p for p in wt_root.iterdir()} if wt_root.exists() else {}
        cx.rec.notes["worktrees"] = sorted(trees)
        def files_in(d):
            return sorted(str(p.relative_to(d)) for p in d.rglob("*") if p.is_file() and ".git" not in p.relative_to(d).parts and ".arbos" not in p.relative_to(d).parts)
        main_tree = next((d for n, d in trees.items() if "from-main" in n), None)
        lag_tree = next((d for n, d in trees.items() if "from-lagging" in n), None)
        cx.rec.expect(len(spawns) == 2 and not any(e.get("error") for e in spawns), "wt-01-spawn-refused", f"a spawn did not succeed: {[e.get('error') for e in spawns]}", "arbos-kernel worktree.rs create_from (#357)")
        if main_tree:
            fm = files_in(main_tree)
            cx.rec.notes["from_main_files"] = fm
            cx.rec.expect(len(fm) >= 9, "wt-01-empty-project", f"the worker cut from a near-empty base sees {len(fm)} file(s): {fm[:5]} — the empty-project shape (#357)", "arbos-kernel worktree.rs create_from")
            said = next((r for r in results if "from-main" in r), "")
            cx.rec.expect("cut from HEAD" in said or ("empty project" in said and "HEAD" in said), "wt-01-result-silent", f"the spawn result does not say the base was replaced by HEAD: {said[:200]!r}", "arbos-kernel worktree.rs note")
        else:
            cx.rec.broke("wt-01-no-worktree", f"no worktree folder for from-main under {wt_root}: {sorted(trees)}")
        if lag_tree:
            fl = files_in(lag_tree)
            cx.rec.notes["from_lagging_files"] = fl
            branch_base = run_git(lag_tree, "merge-base", "HEAD", "lagging")
            lag_sha = run_git(place, "rev-parse", "lagging")
            cx.rec.expect("extra.py" not in " ".join(fl) and branch_base == lag_sha, "wt-01-lagging-base-overridden", f"a base that merely lags HEAD was not used as asked (extra.py present: {'src/extra.py' in fl}, merge-base {branch_base[:8]} vs lagging {lag_sha[:8]})")

    # ── #358 / #354: Stop holds the queued follow-up ──────────────────────
    @reg("sq-01-stop-holds-the-queued-follow-up", tags=("stop", "queue"))
    def sq01(cx):
        """A user line sent while a turn runs is queued (an inbox row, ready). Stop ends the turn and HOLDS the row: `when = waits`, its file kept with `wake = false`, nothing runs on its own; `plan_op run` sends it as its own turn with the words as the prompt (#358). On a kernel without #358 the row is deleted — the user's words accepted and lost."""
        lines = [
            {"agent": "root", "content": "working", "calls": [{"name": "bash", "arguments": {"command": "sleep 30; echo slow", "description": "slow step"}}]},
            {"agent": "root", "content": "Here is the follow-up, answered."},
        ]
        k = cx.kernel(extra_args=["--provider", "replay", "--replies", str(replies_file(cx, lines))])
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        c = k.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        c.user("root", "do the slow thing")
        cx.rec.expect(c.wait_turn("root", "running", 10) is not None, "sq-01-turn-never-started", "no running turn")
        time.sleep(1)
        c.user("root", "then do this next")
        plan = c.wait(lambda f: f.get("type") == "plan" and any(n.get("inbox") for n in f.get("nodes", [])), 10, "the queued row")
        rows = [n for n in (plan or {}).get("nodes", []) if n.get("inbox")]
        cx.rec.notes["queued_rows"] = [{k_: r.get(k_) for k_ in ("id", "goal", "when")} for r in rows]
        cx.rec.expect(bool(rows), "sq-01-not-queued", "a user line during a running turn was not queued as an inbox row")
        inbox = cx.place / ".arbos" / "agents" / "root" / "inbox"
        before = sorted(p.name for p in inbox.iterdir()) if inbox.exists() else []
        c.send({"type": "stop", "agent": "root"})
        cx.rec.expect(c.wait_turn("root", "idle", 15) is not None, "sq-01-stop-did-not-end", "Stop did not end the turn")
        time.sleep(1.5)
        after = sorted(p.name for p in inbox.iterdir()) if inbox.exists() else []
        texts = {n: (inbox / n).read_text(errors="replace") for n in after}
        c2 = k.attach()
        held_plan = c2.wait(lambda f: f.get("type") == "plan" and f.get("agent") == "root", 10, "the plan after Stop")
        held = [n for n in (held_plan or {}).get("nodes", []) if n.get("inbox")]
        cx.rec.notes.update({"inbox_before_stop": before, "inbox_after_stop": after, "held_rows": [{k_: r.get(k_) for k_ in ("id", "goal", "when")} for r in held]})
        evs, _ = transcript(cx.place, "root")
        cx.rec.expect(bool(after), "sq-01-follow-up-deleted", "Stop deleted the queued follow-up: the inbox is empty and the user's words are gone (no file, no line)", "arbos-kernel hooks.rs stop → inbox rows held (#358)")
        if after:
            cx.rec.expect(all("wake = false" in t for t in texts.values()), "sq-01-not-held", f"the kept row is not held (wake = false missing): {list(texts.values())[0][:160]!r}")
            cx.rec.expect(held and all(r.get("when") == "waits" for r in held), "sq-01-row-not-waits", f"the row after Stop should read `waits`, not run on its own: {cx.rec.notes['held_rows']}")
            cx.rec.expect(not any(e.get("kind") == "user" and e.get("text") == "then do this next" for e in evs), "sq-01-ran-on-its-own", "the held follow-up ran without Send now")
            node = (held or rows)[0].get("id")
            c2.send({"type": "plan_op", "agent": "root", "node": node, "op": "run", "text": ""})
            got = c2.wait(lambda f: f.get("type") == "event" and f.get("event", {}).get("kind") == "assistant" and "follow-up, answered" in f.get("event", {}).get("text", ""), 30, "Send now runs it")
            evs, _ = transcript(cx.place, "root")
            at = next((i for i, e in enumerate(evs) if e.get("kind") == "user" and e.get("text") == "then do this next"), None)
            cx.rec.expect(got is not None and at is not None and at > 0 and evs[at - 1].get("kind") == "wake", "sq-01-send-now-not-own-turn", f"Send now did not run the follow-up as its own turn with the words as the prompt (answered={got is not None}, user_at={at})")

    @reg("sq-02-desktop-stop-holds-follow-up", needs_model=True, tags=("stop", "queue", "desktop"))
    def sq02(cx):
        """In the desktop: a follow-up typed while a turn runs, then Stop. What the window shows as held must exist in the kernel's inbox too; while the desktop half (#354) is in and the kernel half (#358) is not, the row is drawn but its file is gone, and Send now sends nothing."""
        if not desktop_available():
            cx.rec.notes["skipped"] = "desktop binary/driver/Xvfb missing"
            return
        from journey_scenarios import Rig

        folder = cx.scratch / "stop-project"
        folder.mkdir(parents=True, exist_ok=True)
        rig = Rig(cx, [folder], tag="app-stop")
        try:
            time.sleep(3)
            rig.focus(folder)
            # Staging this needs a turn that is *still running* when the follow-up is typed, and
            # the model decides whether that happens. Asked to `sleep 40` inline, it sometimes
            # obeys and sometimes routes the work to a worker — which the coordinator protocol
            # tells it to do ("keep the chat responsive, route substantial work to workers"), so
            # the turn is over in two seconds. Measured 2026-09-19 across six runs: three held the
            # turn 10.8 s, three ended it in 2.2-2.4 s with "I cannot run bash commands longer
            # than a few seconds as a coordinator" (qal-j47).
            #
            # `wait_busy` is not enough on its own: the turn *starts* in both cases. What matters
            # is that it is still up a moment later, when the line is typed. So check that, and
            # try again when the model chose the other reading — three tries turns a coin-flip
            # into something the cycle can rely on.
            inbox = folder / ".arbos" / "agents" / "root" / "inbox"
            staged = False
            for attempt in range(1, 4):
                rig.wait_idle(folder, 60)
                rig.send("Run `sleep 40; echo slow` with bash, then say done.")
                if not rig.wait_busy(folder, 20):
                    cx.rec.notes.setdefault("staging", []).append(f"attempt {attempt}: the turn never started")
                    continue
                # Past the refusal window, not just past the start. Measured: a turn the model
                # declines ends at 2.2-4.0 s, one it accepts runs to ~10.8 s. A three-second check
                # sat inside that spread and passed runs that died at 3.7 s, which then looked like
                # the queue failing. Six seconds is clear of every refusal seen and still leaves
                # the best part of five seconds to type and press.
                time.sleep(6)
                if rig.busy(folder):
                    staged = True
                    cx.rec.notes["staged_on_attempt"] = attempt
                    break
                cx.rec.notes.setdefault("staging", []).append(
                    f"attempt {attempt}: the turn ended before the follow-up could be typed (the model routed the work to a worker)"
                )
            cx.rec.expect(
                staged,
                "sq-02-could-not-hold-a-turn",
                "three tries and the model never held a turn open long enough to queue behind, so the "
                "queued-follow-up path could not be reached (qal-j47 — a staging failure, not a product fault)",
            )
            if not staged:
                return
            # Enter during a turn steers (#362 makes the command yield to it); the QUEUED follow-up is
            # cmd/ctrl-shift-enter — "queue next" — the row under the composer that Stop used to delete.
            rig.app.wait_element("composer-field", reachable=True)
            rig.app.click("composer-field")
            rig.app.type("Then reply with the single word FOLLOWUP.")
            # The turn can still end between staging and the keystroke, and a line queued into a
            # finished turn opens its own turn instead — no inbox row, and it reads as the queue
            # failing. Record what was true at the moment the keys went down, so a future failure
            # can be told apart from this one without re-deriving it (qal-j47).
            cx.rec.notes["turn_still_running_at_keypress"] = rig.busy(folder)
            queued_files = []
            for combo in ("ctrl-shift-enter", "cmd-shift-enter"):
                try:
                    rig.app.key(combo)
                except Exception as e:  # noqa: BLE001
                    cx.rec.notes.setdefault("key_errors", []).append(f"{combo}: {str(e)[:80]}")
                    continue
                end = time.time() + 6
                while time.time() < end and not queued_files:
                    queued_files = sorted(p.name for p in inbox.iterdir()) if inbox.exists() else []
                    time.sleep(0.5)
                if queued_files:
                    cx.rec.notes["queued_with"] = combo
                    break
            rig.pulse("queue next")
            if not queued_files:
                cx.rec.notes["skipped"] = "the follow-up could not be queued from the desktop (no inbox row after ctrl/cmd-shift-enter); the queued-follow-up path was not exercised"
                return
            if not rig.busy(folder):
                cx.rec.notes["skipped"] = "the turn had already ended before Stop could be pressed; the Stop-holds-queue path was not exercised"
                return
            rig.app.click("composer-stop")
            rig.pulse("Stop with a follow-up queued")
            cx.rec.expect(rig.wait_idle(folder, 20), "sq-02-stop-did-not-end", "Stop did not end the turn")
            time.sleep(2)
            files_after = sorted(p.name for p in inbox.iterdir()) if inbox.exists() else []
            chat = rig.root_chat(folder) or {}
            # The window's own account of the queue: `held` = the kernel's inbox rows it draws under the composer
            # (#354), `queued` = lines still on the window's side of the wire.
            held = chat.get("held") or 0
            queued_local = chat.get("queued") or 0
            cx.rec.notes.update({"queued_files_before_stop": queued_files, "files_after_stop": files_after, "held_shown_after_stop": held, "queued_local_after_stop": queued_local})
            window_holds = (held + queued_local) > 0
            kernel_holds = bool(files_after)
            cx.rec.expect(kernel_holds, "sq-02-kernel-deleted-follow-up", "the kernel deleted the queued follow-up on Stop (its inbox is empty)", "arbos-kernel hooks.rs (#358)")
            cx.rec.expect(window_holds == kernel_holds, "sq-02-halves-disagree", f"the window and the kernel disagree after Stop: window shows the follow-up={window_holds}, kernel holds it={kernel_holds} — a Send now would send nothing", "desktop #354 vs kernel #358")
            # Nothing runs on its own after Stop.
            time.sleep(5)
            evs = read_transcript(folder)
            cx.rec.expect(not any(e.get("kind") == "user" and "FOLLOWUP" in e.get("text", "") for e in evs), "sq-02-ran-on-its-own", "the held follow-up ran by itself after Stop")
        finally:
            rig.close(folders=[folder])
            cx.rec.snapshot(folder, "stop-after")


    # ── #362: the impatient user ──────────────────────────────────────────
    @reg("im-01-bash-yields-to-the-users-words", tags=("impatient",))
    def im01(cx):
        """An attached bash streams its output as `job` frames while it holds the turn; a user line (steer) while it runs is answered within 8 s, not after the command; the same line sent twice is one user line plus an "Already queued … not added again" notice."""
        lines = [
            {"agent": "root", "content": "running it", "calls": [{"name": "bash", "arguments": {"command": "for i in $(seq 1 40); do echo tick $i; sleep 0.5; done", "description": "ticks"}}]},
            {"agent": "root", "content": "It is running — 20 seconds in, still going; I will report when it ends."},
        ]
        k = cx.kernel(extra_args=["--provider", "replay", "--replies", str(replies_file(cx, lines))])
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        c = k.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        c.user("root", "start it")
        cx.rec.expect(c.wait_turn("root", "running", 10) is not None, "im-01-turn-never-started", "no running turn")
        job = c.wait(lambda f: f.get("type") == "job" and f.get("agent") == "root" and "tick" in str(f.get("delta", "")), 10, "a job frame with output")
        cx.rec.notes["first_job_frame"] = {k_: str(v)[:60] for k_, v in (job or {}).items()}
        cx.rec.expect(job is not None and job.get("running") is True, "im-01-no-job-stream", "the attached command's output did not arrive as `job` frames while it ran", "arbos-kernel serve.rs / bash.rs (#362)")
        time.sleep(1.5)
        asked = time.time()
        c.user("root", "run it", steer=True)
        c.user("root", "run it", steer=True)
        reply = c.wait(lambda f: f.get("type") == "event" and f.get("agent") == "root" and f.get("event", {}).get("kind") == "assistant" and str(f.get("event", {}).get("text", "")).startswith("It is running"), 12, "the turn answers while the command runs")
        took = round(time.time() - asked, 1)
        cx.rec.notes["answered_after_s"] = took
        cx.rec.expect(reply is not None and took < 8, "im-01-typing-unanswered", f"the user's words were not answered while the command ran (answered={reply is not None}, after {took} s; the command itself runs 20 s)", "arbos-kernel bash.rs yield (#362)")
        time.sleep(1)
        evs, _ = transcript(cx.place, "root")
        runs = [e for e in evs if e.get("kind") == "user" and e.get("text") == "run it"]
        dup = [e for e in evs if e.get("kind") == "notice" and str(e.get("text", "")).startswith("Already queued: \"run it\"")]
        cx.rec.notes.update({"run_it_lines": len(runs), "already_queued_notice": [e.get("text", "")[:120] for e in dup]})
        cx.rec.expect(len(runs) == 1, "im-01-repeat-stacked", f"the same line typed twice is on the transcript {len(runs)} time(s)", "arbos-core inbox.rs dedupe (#362)")
        cx.rec.expect(bool(dup) and "not added again" in dup[0].get("text", ""), "im-01-repeat-unacknowledged", "the repeat was dropped without the 'Already queued … not added again' notice")

    @reg("im-02-desktop-no-quiet-line-while-streaming", needs_model=True, tags=("impatient", "desktop"))
    def im02(cx):
        """In the desktop: a command that prints every second for 80 s holds the turn; the stall hint ("Nothing has arrived in …, check the model key") must not appear while output streams (it did twice on Jacob's screen). Control: a command that prints nothing for 80 s — the hint appears after a minute and names the command, never the model key."""
        if not desktop_available():
            cx.rec.notes["skipped"] = "desktop binary/driver/Xvfb missing"
            return
        from journey_scenarios import Rig

        folder = cx.scratch / "impatient-project"
        folder.mkdir(parents=True, exist_ok=True)
        rig = Rig(cx, [folder], tag="app-impatient")
        try:
            time.sleep(3)
            rig.focus(folder)
            rig.wait_idle(folder, 60)

            def watch(seconds):
                seen = []
                end = time.time() + seconds
                while time.time() < end:
                    try:
                        hint = rig.app.find("stall-hint")
                    except Exception:  # noqa: BLE001
                        hint = None
                    if hint is None:
                        # The hint may sit under a longer path; look for it among all elements.
                        try:
                            hint = next((e for e in rig.app.elements("*") if "stall" in str(e.get("path", ""))), None)
                        except Exception:  # noqa: BLE001
                            hint = None
                    if hint:
                        seen.append((round(seconds - (end - time.time())), {k_: str(v)[:100] for k_, v in hint.items() if k_ in ("text", "label", "path")}))
                    if not rig.busy(folder):
                        break
                    time.sleep(3)
                return seen

            # Streaming: no hint may appear.
            rig.send("Run exactly this with bash: `for i in $(seq 1 80); do echo tick $i; sleep 1; done`. Then say done.")
            cx.rec.expect(rig.wait_busy(folder, 20), "im-02-turn-never-started", "the streaming command's turn never started")
            seen = watch(85)
            rig.pulse("after 85 s of a streaming command")
            cx.rec.notes["hint_while_streaming"] = seen[:3]
            cx.rec.expect(not seen, "im-02-quiet-line-over-a-streaming-command", f"the stall hint appeared while the command was printing every second: {seen[:2]}", "desktop transcript.rs stall hint / session.rs quiet_for counts job frames (#362)")
            rig.wait_idle(folder, 60)
            # Silent: the hint appears after a minute and names the command, not the key.
            rig.send("Run exactly this with bash: `sleep 80`. Then say done.")
            cx.rec.expect(rig.wait_busy(folder, 20), "im-02-silent-turn-never-started", "the silent command's turn never started")
            seen = watch(85)
            rig.pulse("after 85 s of a silent command")
            cx.rec.notes["hint_while_silent"] = seen[:3]
            texts = " ".join(str(h.get("text", "")) + str(h.get("label", "")) for _, h in seen)
            if not seen:
                cx.rec.notes["silent_control"] = "unverified: no stall-hint element surfaced in 85 s of a silent command — either the hint did not appear (its clock counts the running command as progress) or the driver does not list it; the streaming half above is the assertion that matters"
            if seen:
                cx.rec.expect("model key" not in texts.lower() and "nothing has arrived" not in texts.lower(), "im-02-silent-command-blamed-on-the-key", f"the hint over a silent command sends the user to the model key: {texts[:160]!r}")
                cx.rec.expect("printed nothing" in texts.lower() or "sleep" in texts.lower() or texts.strip() == "", "im-02-hint-does-not-name-the-command", f"the hint does not name the running command: {texts[:160]!r}")
            rig.wait_idle(folder, 60)
        finally:
            rig.close(folders=[folder])
            cx.rec.snapshot(folder, "impatient-after")


    # ── #371: the kernel deaf to its own children ──────────────────────────
    def job_result_arrives(cx, k, tag, budget_s=20):
        c = k.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        t0 = time.time()
        c.user("root", f"Run `sleep 2; echo done-{tag}` with bash and tell me what it printed.")
        cx.rec.expect(c.wait_turn("root", "running", 10) is not None, "turn-never-started", "no running turn")
        idle = c.wait_turn("root", "idle", budget_s)
        took = round(time.time() - t0, 1)
        evs, _ = transcript(cx.place, "root")
        tools = [e for e in evs if e.get("kind") == "tool" and e.get("name") == "bash"]
        body = " ".join(str(e.get("body") or e.get("result") or "") for e in tools)
        still_running = "still running" in body.lower()
        # A zombie under the kernel is the shape Jacob's process table showed.
        zombies = 0
        try:
            out = subprocess.run(["ps", "-o", "pid=,stat=,ppid=,comm=", "--ppid", str(k.proc.pid)], capture_output=True, text=True).stdout
            zombies = sum(1 for l in out.splitlines() if l.split() and len(l.split()) > 1 and l.split()[1].startswith("Z"))
        except Exception:  # noqa: BLE001
            pass
        return {"idle": idle is not None, "took_s": took, "tool_lines": len(tools), "result_has_output": f"done-{tag}" in body, "still_running": still_running, "zombies_under_kernel": zombies, "body": body[:200]}

    @reg("rp-01-finished-job-result-with-reaper-broken", tags=("jobs", "platform"))
    def rp01(cx):
        """With the runtime's child reaper disabled (ARBOS_TEST_NO_CHILD_WAIT), a two-second bash must still return its result within seconds — the wrapper's `exit` file is the truth about a command — not sit as a zombie and come back "still running" at the 600 s floor (#371)."""
        tag = f"R{now_ms() % 100000}"
        cx.env["ARBOS_TEST_NO_CHILD_WAIT"] = "1"
        lines = [
            {"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": f"sleep 2; echo done-{tag}", "description": "a short job"}}]},
            {"agent": "root", "content": f"It printed done-{tag}."},
        ]
        k = cx.kernel(extra_args=["--provider", "replay", "--replies", str(replies_file(cx, lines))])
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        r = job_result_arrives(cx, k, tag, budget_s=25)
        cx.rec.notes.update(r)
        cx.rec.expect(r["idle"] and r["result_has_output"] and not r["still_running"], "rp-01-result-never-arrived", f"with the reaper broken the finished job produced no result within 25 s (idle={r['idle']}, output seen={r['result_has_output']}, still_running={r['still_running']}) — the exit-file path did not deliver", "arbos-engine bash.rs / jobs.rs exit file + reap_by_pid (#371)")
        cx.rec.expect(r["zombies_under_kernel"] == 0, "rp-01-zombie-left", f"{r['zombies_under_kernel']} zombie child(ren) under the kernel after the job ended (reap_by_pid did not run)")

    @reg("rp-02-sigchld-blocked-by-the-launcher", tags=("jobs", "platform"))
    def rp02(cx):
        """The kernel is exec'd with SIGCHLD blocked in its inherited signal mask (what a launcher can do; the shape behind Jacob's five hung workers on macOS). The kernel must unblock it and say so on stderr, and a job must still end with its result. On Linux the pidfd reaper would have masked the fault, which is why a week of green cycles never saw it."""
        import signal as _signal

        tag = f"S{now_ms() % 100000}"

        def block_sigchld():
            _signal.pthread_sigmask(_signal.SIG_BLOCK, {_signal.SIGCHLD})

        lines = [
            {"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": f"sleep 2; echo done-{tag}", "description": "a short job"}}]},
            {"agent": "root", "content": f"It printed done-{tag}."},
        ]
        k = cx.kernel(preexec=block_sigchld, extra_args=["--provider", "replay", "--replies", str(replies_file(cx, lines))])
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up with SIGCHLD blocked")
        r = job_result_arrives(cx, k, tag, budget_s=25)
        stderr = k.stderr_text() if hasattr(k, "stderr_text") else ""
        said = "SIGCHLD" in stderr
        cx.rec.notes.update(r)
        cx.rec.notes["kernel_said_unblocked"] = said
        cx.rec.expect(r["idle"] and r["result_has_output"], "rp-02-result-never-arrived", f"a job under a kernel exec'd with SIGCHLD blocked produced no result within 25 s ({r})", "arbos-kernel main.rs SIGCHLD unblock (#371)")
        cx.rec.expect(said, "rp-02-mask-not-reported", "the kernel did not say on stderr that SIGCHLD was blocked and unblocked — on a build without #371 the mask stays and only Linux's pidfd hides it")


    # ── #374: the turn watchdog ────────────────────────────────────────────
    @reg("st-01-silent-turn-names-its-wait", tags=("watchdog",))
    def st01(cx):
        """A turn that shows nothing for ARBOS_STALL_SECS (5 min in production; 3 s here) gets one plain notice — "Still working …" — naming the tool in flight and its command and what the user can do; not a failure, said once; the turn then ends normally when the command does."""
        cx.env["ARBOS_STALL_SECS"] = "3"
        lines = [
            {"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": "sleep 12", "description": "a long quiet command"}}]},
            {"agent": "root", "content": "Done waiting."},
        ]
        k = cx.kernel(extra_args=["--provider", "replay", "--replies", str(replies_file(cx, lines))])
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        c = k.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        c.user("root", "Wait quietly for twelve seconds, then say done.")
        cx.rec.expect(c.wait_turn("root", "running", 10) is not None, "st-01-turn-never-started", "no running turn")
        stall = c.wait(lambda f: f.get("type") == "event" and f.get("agent") == "root" and f.get("event", {}).get("kind") == "notice" and str(f.get("event", {}).get("text", "")).startswith("Still working"), 20, "the stall notice")
        text = str((stall or {}).get("event", {}).get("text", ""))
        cx.rec.notes["stall_notice"] = text[:240]
        cx.rec.expect(stall is not None, "st-01-no-notice", "a turn silent for 3 s (+ the 5 s tick) produced no 'Still working' notice within 20 s", "arbos-kernel hooks.rs stall watchdog (#374)")
        if stall:
            cx.rec.expect((stall.get("event") or {}).get("failed") is False, "st-01-notice-marked-failed", "the stall notice is marked failed; it is a report, not a failure")
            cx.rec.expect("`bash`" in text and "sleep 12" in text, "st-01-tool-unnamed", f"the notice does not name the tool and its command: {text[:160]!r}")
            cx.rec.expect("Stop ends the turn" in text, "st-01-no-way-out", f"the notice does not say what the user can do: {text[:160]!r}")
        cx.rec.expect(c.wait_turn("root", "idle", 40) is not None, "st-01-turn-never-ended", "the turn did not end after the command did")
        time.sleep(1)
        evs, _ = transcript(cx.place, "root")
        stalls = [e for e in evs if e.get("kind") == "notice" and str(e.get("text", "")).startswith("Still working")]
        cx.rec.notes["stall_notices_on_transcript"] = len(stalls)
        cx.rec.expect(len(stalls) == 1, "st-01-said-more-than-once", f"the stall line is on the transcript {len(stalls)} time(s) for one silence")
        cx.rec.expect(any(e.get("kind") == "assistant" and "Done waiting" in e.get("text", "") for e in evs) and evs[-1].get("kind") == "turn_complete", "st-01-turn-did-not-finish-normally", "after the notice the turn did not finish with its reply and turn_complete")

    @reg("pn-01-panic-does-not-strand-the-agent", tags=("watchdog", "platform"))
    def pn01(cx):
        """A panic on the turn's own task (ARBOS_TEST_PANIC_TURN=root fires once): the agent goes idle within seconds instead of staying `running` for good; the transcript ends with a failed notice naming the internal error and a `turn_complete`; kernel.log says `turn_panicked`; the next message runs a normal turn (#374). Before the guard a user saw only a working line, forever, with no error anywhere."""
        cx.env["ARBOS_TEST_PANIC_TURN"] = "root"
        cx.rec.notes["expected_panic"] = "the turn task panicked on purpose"  # the harness's panic detector lets this one through
        lines = [{"agent": "root", "content": "Fine now."}]
        k = cx.kernel(extra_args=["--provider", "replay", "--replies", str(replies_file(cx, lines))])
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        c = k.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        c.user("root", "first")
        cx.rec.expect(c.wait_turn("root", "running", 10) is not None, "pn-01-turn-never-started", "no running turn")
        idle = c.wait_turn("root", "idle", 12)
        evs, _ = transcript(cx.place, "root")
        notice = next((e for e in evs if e.get("kind") == "notice" and e.get("failed")), None)
        log = ""
        for pth in (cx.place / ".arbos" / "runtime" / "kernel.log", cx.place / ".arbos" / "kernel.log"):
            if pth.exists():
                log = pth.read_text(errors="replace")
        cx.rec.notes.update({"idle_after_panic": idle is not None, "notice": (notice or {}).get("text", "")[:200], "ends_with": evs[-1].get("kind") if evs else None, "turn_panicked_logged": '"event":"turn_panicked"' in log})
        cx.rec.expect(idle is not None, "pn-01-agent-stranded", "the agent stayed `running` after its turn task panicked — the shape that looks exactly like the app thinking", "arbos-kernel sched.rs turn guard (#374)")
        cx.rec.expect(notice is not None and "internal error" in notice.get("text", "").lower(), "pn-01-no-reason-on-record", f"no failed notice naming the internal error on the transcript: {(notice or {}).get('text', '')[:120]!r}")
        cx.rec.expect(bool(evs) and evs[-1].get("kind") == "turn_complete", "pn-01-record-not-closed", f"the transcript ends in {evs[-1].get('kind') if evs else None}, not turn_complete")
        cx.rec.expect('"event":"turn_panicked"' in log, "pn-01-not-logged", "kernel.log has no turn_panicked event")
        c.user("root", "second")
        cx.rec.expect(c.wait_turn("root", "running", 10) is not None and c.wait_turn("root", "idle", 20) is not None, "pn-01-next-message-stuck", "the next message did not run a normal turn after the panic")
        evs, _ = transcript(cx.place, "root")
        cx.rec.expect(any(e.get("kind") == "assistant" and e.get("text") == "Fine now." for e in evs), "pn-01-next-message-unanswered", "the message after the panic was not answered")


    # ── #377: the incident shape, pinned ───────────────────────────────────
    def procs_writing_under(place, needle):
        """Processes that name `needle` in their argv or have a cwd under `place` — the writer and its shells."""
        found = []
        try:
            out = subprocess.run(["ps", "-eo", "pid=,ppid=,stat=,args="], capture_output=True, text=True, timeout=10).stdout
        except Exception:  # noqa: BLE001
            return found
        for line in out.splitlines():
            parts = line.split(None, 3)
            if len(parts) < 4:
                continue
            pid, ppid, stat, args = parts
            cwd = ""
            try:
                cwd = os.readlink(f"/proc/{pid}/cwd")
            except OSError:
                pass
            if needle in args or cwd.startswith(str(place)):
                if "run.py" in args or " serve " in args or "arbos-kernel" in args.split()[0]:
                    continue  # the kernel and the harness are not the job
                found.append({"pid": int(pid), "ppid": int(ppid), "stat": stat, "args": args[:90], "cwd_under_place": cwd.startswith(str(place))})
        return found

    @reg("jl-01-kill-the-displayed-pid-ends-the-group", tags=("jobs", "leash"))
    def jl01(cx):
        """The 164 GB incident, step for step: a background flood (`while true; do echo …; done`), `jobs`, then `kill <the pid jobs shows>`. Afterwards nothing under the place may be running or writing: the flood's shell and its loop are gone, out.log has stopped growing, and `jobs` says the job ended. Before #377 the displayed pid was the wrapper/leash: killing it orphaned the writer, which outlived the kernel and its folder."""
        marker = f"flood-{now_ms() % 100000}"
        flood = f'while true; do echo "{marker} runaway line"; done'
        lines = [
            {"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"background": True, "command": flood, "description": "a slow flood"}}]},
            {"agent": "root", "content": "", "calls": [{"name": "jobs", "arguments": {}}]},
            # The pid `jobs` shows is the one in meta.json; the model kills exactly that, through its own shell, as it did.
            {"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": "kill $(python3 -c 'import json;print(json.load(open(\".arbos/agents/root/jobs/j1/meta.json\"))[\"pid\"])')", "description": "kill the job by its shown pid"}}]},
            {"agent": "root", "content": "", "calls": [{"name": "jobs", "arguments": {}}]},
            {"agent": "root", "content": "Killed it."},
        ]
        k = cx.kernel(extra_args=["--provider", "replay", "--replies", str(replies_file(cx, lines))])
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        c = k.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        c.user("root", "Start a runaway flood in the background, list jobs, then kill it by the pid you see.")
        cx.rec.expect(c.wait_turn("root", "idle", 60) is not None, "jl-01-turn-never-ended", "the turn never ended")
        time.sleep(2)
        out_log = cx.place / ".arbos" / "agents" / "root" / "jobs" / "j1" / "out.log"
        size1 = out_log.stat().st_size if out_log.exists() else -1
        time.sleep(3)
        size2 = out_log.stat().st_size if out_log.exists() else -1
        left = procs_writing_under(cx.place, marker)
        evs, _ = transcript(cx.place, "root")
        jobs_said = [str(e.get("body") or e.get("result") or "")[:200] for e in evs if e.get("kind") == "tool" and e.get("name") == "jobs"]
        kill_said = [str(e.get("body") or e.get("result") or "")[:200] for e in evs if e.get("kind") == "tool" and e.get("name") == "bash" and "kill" in json.dumps(e.get("args", {}))]
        cx.rec.notes.update({"out_log_bytes": [size1, size2], "still_running": left, "jobs_said": jobs_said, "kill_said": kill_said})
        cx.rec.expect(not left, "jl-01-writer-survived", f"after `kill <shown pid>` {len(left)} process(es) of the job are still alive: {left[:3]} — the 164 GB shape", "arbos-engine jobs.rs / bash.rs: a signal at the displayed pid ends the group (#377)")
        cx.rec.expect(size1 >= 0 and size2 == size1, "jl-01-log-still-growing", f"out.log grew from {size1} to {size2} bytes in 3 s after the kill — something is still writing")
        cx.rec.expect(bool(jobs_said) and any(("killed" in j.lower() or "exit" in j.lower()) for j in jobs_said[-1:]), "jl-01-jobs-still-says-running", f"`jobs` after the kill does not say the job ended: {jobs_said[-1:] }")
        # Whatever survived is ended here too, so the rig itself never repeats the incident.
        for pr in left:
            try:
                os.kill(pr["pid"], signal.SIGKILL)
            except Exception:  # noqa: BLE001
                pass

    # ── #377's stated cost: a backgrounding subscription command ───────────
    @reg("sb-01-backgrounding-subscription-command-finishes", tags=("jobs", "subscriptions"))
    def sb01(cx):
        """A `kind = shell` subscription whose command backgrounds something (`nohup sleep 300 &`) and exits must be run to completion promptly — its reading delivered within seconds — not held until the command's timeout because the leash waits on the group. #377's author says this needs #364 beside it; here it is measured."""
        from fileplan_scenarios import write_subscription

        write_subscription(cx.place, "root", "bg", kind="shell", cmd="nohup sleep 300 >/dev/null 2>&1 & echo started-bg", every="30s", deliver_to="user", notify="bg: {output}")
        k = cx.kernel()
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        c = k.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        t0 = time.time()
        # The reading is a delivered line ("bg: started-bg"), never the subscription file echoed in a snapshot or plan
        # frame — matching the raw frame text would pass on the definition alone.
        def is_reading(f):
            if f.get("type") in ("snapshot", "plan", "hello"):
                return False
            text = json.dumps(f.get("event", f))
            return "bg: started-bg" in text
        told = c.wait(is_reading, int(os.environ.get("ARBOS_QA_SB01_WAIT", "45")), "the subscription's reading")
        cx.rec.notes["reading_frame"] = {kk: str(v)[:80] for kk, v in (told or {}).items()}
        took = round(time.time() - t0, 1)
        cx.rec.notes.update({"reading_delivered": told is not None, "took_s": took})
        cx.rec.expect(told is not None, "sb-01-held-until-timeout", f"a shell subscription whose command backgrounds a child did not deliver its reading within 45 s (the command itself exits at once) — held on the backgrounded child", "arbos-kernel plan.rs shell run + jobs.rs group wait (#377 vs #364)")
        # Clean the backgrounded sleep so it does not outlive the scenario.
        subprocess.run(["pkill", "-f", "^sleep 300$"], capture_output=True)


    # ── rewind keeps the history (the standing_pass_e2e lead) ──────────────
    import threading

    # ── the general property: a rewind that fails leaves the tree where it was ──────────
    def tree_state(place):
        """Every file outside .arbos/ and .git/ with its bytes, plus HEAD and the index: what the person has."""
        files = {}
        for p_ in sorted(place.rglob("*")):
            rel = p_.relative_to(place)
            if rel.parts and rel.parts[0] in (".arbos", ".git"):
                continue
            if p_.is_file():
                files[str(rel)] = p_.read_bytes()
        git_ = lambda *a: subprocess.run(["git", *a], cwd=place, capture_output=True, text=True).stdout.strip()
        return {"files": files, "head": git_("rev-parse", "HEAD"), "index": git_("ls-files", "-s")}

    def tree_diff(before, after):
        out = []
        for f in sorted(set(before["files"]) | set(after["files"])):
            if f not in after["files"]:
                out.append(f"{f}: GONE")
            elif f not in before["files"]:
                out.append(f"{f}: NEW")
            elif before["files"][f] != after["files"][f]:
                out.append(f"{f}: CHANGED ({before['files'][f][:40]!r} -> {after['files'][f][:40]!r})")
        if before["head"] != after["head"]:
            out.append(f"HEAD: {before['head'][:10]} -> {after['head'][:10]}")
        if before["index"] != after["index"]:
            out.append("index: changed")
        return out

    def rewind_round(cx, k, c, keep_texts, rewind_turn, sample_s=8.0):
        """Send one rewind with files: true, sample the transcript file every 50 ms for sample_s, and return what
        a person could have seen: the lowest line count observed, any read that came back empty, the settled
        transcript, and whether every kept turn is still there."""
        tr = cx.place / ".arbos" / "agents" / "root" / "transcript.jsonl"
        samples = []
        stop = threading.Event()

        def sampler():
            while not stop.is_set():
                try:
                    text = tr.read_text(errors="replace")
                    n = sum(1 for l in text.splitlines() if l.strip())
                    samples.append((round(time.time(), 3), n, len(text)))
                except FileNotFoundError:
                    samples.append((round(time.time(), 3), -1, -1))
                except Exception:  # noqa: BLE001
                    samples.append((round(time.time(), 3), -2, -2))
                time.sleep(0.05)

        th = threading.Thread(target=sampler, daemon=True)
        before_n = sum(1 for l in tr.read_text(errors="replace").splitlines() if l.strip())
        tree_before = tree_state(cx.place)
        th.start()
        t0 = time.time()
        c.send({"type": "rewind", "agent": "root", "turn": rewind_turn, "files": True})
        first = c.wait(lambda f: f.get("type") == "rewound" and f.get("agent") == "root", 15, "the rewound frame")
        follow = c.wait(lambda f: (f.get("type") == "rewound" and f.get("restored") is not None) or f.get("type") == "error", 30, "the restore's report")
        # Settle: the transcript must end on a finished turn; poll, never infer.
        settled = None
        end = time.time() + 10
        while time.time() < end:
            evs, _ = transcript(cx.place, "root")
            if evs and evs[-1].get("kind") == "turn_complete":
                settled = evs
                break
            time.sleep(0.1)
        while time.time() - t0 < sample_s:
            time.sleep(0.05)
        stop.set()
        th.join(timeout=2)
        evs, _ = transcript(cx.place, "root")
        texts = [e.get("text", "") for e in evs if e.get("kind") in ("user", "assistant")]
        missing = [t for t in keep_texts if t not in texts]
        nums = [n for _, n, _ in samples if n >= 0]
        failed = bool(follow) and follow.get("type") == "error"
        return {
            # The general property (2026-09-17, after #419): a restore that failed must leave the tree as it was.
            "restore_failed": failed,
            "tree_changed_by_failed_restore": tree_diff(tree_before, tree_state(cx.place)) if failed else [],
            "before_lines": before_n,
            "after_lines": len(evs),
            "rewound_ms": round((first.get("_at", time.time()) - t0) * 1000) if first else None,
            "restore_reported": (follow or {}).get("type"),
            "restore_error": (follow or {}).get("detail") or (follow or {}).get("message") if follow and follow.get("type") == "error" else None,
            "samples": len(samples),
            "min_lines_seen": min(nums) if nums else None,
            "empty_reads": sum(1 for n in nums if n == 0),
            "missing_reads": sum(1 for _, n, _ in samples if n == -1),
            "settled_on_turn_complete": settled is not None,
            "kept_turns_missing_after": missing,
            "ends_with": evs[-1].get("kind") if evs else None,
        }

    def rw_scenario(name, doc, load):
        @reg(name, tags=("rewind", "history"))
        def rw(cx):
            # Three turns, each writing a file, in a place that is a git repository (checkpoints need a commit).
            place = cx.place
            place.mkdir(parents=True, exist_ok=True)
            for args in (["init", "-q"], ["config", "user.name", "qa"], ["config", "user.email", "qa@qa"], ["commit", "-q", "--allow-empty", "-m", "start"]):
                subprocess.run(["git", *args], cwd=place, capture_output=True)
            if os.environ.get("ARBOS_QA_RW_NO_IDENTITY") or load == "no-identity":
                # Variant: a repository with no git identity — commit-tree fails, and the checkpoint silently has no work tree.
                subprocess.run(["git", "config", "--unset", "user.name"], cwd=place, capture_output=True)
                subprocess.run(["git", "config", "--unset", "user.email"], cwd=place, capture_output=True)
            (place / ".gitignore").write_text(".arbos/\n")
            subprocess.run(["git", "add", ".gitignore"], cwd=place, capture_output=True)
            subprocess.run(["git", "-c", "user.name=qa", "-c", "user.email=qa@qa", "commit", "-q", "-m", "ignore .arbos"], cwd=place, capture_output=True)
            replies = []
            for i, word in enumerate(("first", "second", "third", "fourth", "fifth"), 1):
                # bash, not write: a coordinator root refuses to write project files itself.
                replies.append({"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": f"echo {word} > f{i}.txt", "description": f"write f{i}"}}]})
                replies.append({"agent": "root", "content": word})
            spinners = []
            churn = None
            preexec = None
            if load == "pinned":
                # The e2e's failing shape: the kernel on one core, four spinners on the same core.
                def pin():
                    os.sched_setaffinity(0, {0})
                preexec = pin
                for _ in range(4):
                    spinners.append(subprocess.Popen(["taskset", "-c", "0", "sh", "-c", "while :; do :; done"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
            elif load == "churn":
                churn = subprocess.Popen(["sh", "-c", "while :; do dd if=/dev/zero of=" + str(cx.scratch / "churn.bin") + " bs=1M count=200 conv=fsync 2>/dev/null; sync; done"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                for _ in range(max(2, os.cpu_count() or 2)):
                    spinners.append(subprocess.Popen(["sh", "-c", "while :; do :; done"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
            try:
                k = cx.kernel(preexec=preexec, extra_args=["--provider", "replay", "--replies", str(replies_file(cx, replies))])
                cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
                c = k.attach()
                c.wait(lambda f: f.get("type") == "snapshot", 5)
                rounds = []
                # Round 1: turns one, two, three → rewind to turn 3 (keep one, two). Round 2: turns four, five → rewind to turn 4.
                for texts, rewind_turn, keep in ((("one", "two", "three"), 3, ("one", "first", "two", "second")), (("four", "five"), 4, ("one", "first", "two", "second", "four", "fourth"))):
                    for t in texts:
                        c.user("root", t)
                        cx.rec.expect(c.wait_turn("root", "idle", 60) is not None, f"{name}-turn-never-ended", f"turn {t!r} never ended")
                    r = rewind_round(cx, k, c, keep, rewind_turn)
                    r["files_after"] = sorted(p_.name for p_ in place.glob("f*.txt"))
                    cps = (place / ".arbos" / "agents" / "root" / "checkpoints.jsonl")
                    r["checkpoints_with_work_tree"] = sum(1 for l in cps.read_text().splitlines() if '"work"' in l) if cps.exists() else None
                    r["checkpoints"] = sum(1 for l in cps.read_text().splitlines() if l.strip()) if cps.exists() else None
                    rounds.append(r)
                    if r["kept_turns_missing_after"] or r["empty_reads"] or r["missing_reads"]:
                        break
                cx.rec.notes["rounds"] = rounds
                cx.rec.notes["load"] = load
                # What a fresh window is handed: history for root must carry the kept turns.
                # A fresh attach is handed the history as `replayed` events with the snapshot (and `history` pages
                # more on request): everything the new client received is what its window would draw.
                c2 = k.attach()
                c2.wait(lambda f: f.get("type") == "snapshot", 5)
                c2.send({"type": "history", "agent": "root", "limit": 50, "before": 10**9})
                c2.wait(lambda f: f.get("type") == "history_end" and f.get("agent") == "root", 10, "history_end")
                time.sleep(0.5)
                shown = " ".join(json.dumps(f) for _, f in list(c2.frames) if f.get("type") in ("replayed", "event", "snapshot"))
                kept_all = rounds[-1]["kept_turns_missing_after"] == [] if rounds else False
                window_has = [t for t in ("one", "first", "two", "second") if t in shown]
                cx.rec.notes["window_history_has"] = window_has
                for i, r in enumerate(rounds, 1):
                    cx.rec.expect(not r["kept_turns_missing_after"], f"{name}-history-lost", f"round {i}: after the rewind the transcript no longer holds {r['kept_turns_missing_after']} (before {r['before_lines']} lines, after {r['after_lines']}, min seen {r['min_lines_seen']})", "arbos-kernel serve rewind / arbos-engine git restore")
                    cx.rec.expect(r["empty_reads"] == 0 and r["missing_reads"] == 0, f"{name}-history-briefly-gone", f"round {i}: the transcript read empty {r['empty_reads']} time(s) / missing {r['missing_reads']} time(s) during the rewind ({r['samples']} reads at 50 ms) — what a person sees as their history vanishing, even if it comes back")
                    cx.rec.expect(r["settled_on_turn_complete"], f"{name}-not-settled", f"round {i}: the transcript did not end on turn_complete within 10 s of the rewind (ends with {r['ends_with']})")
                    cx.rec.expect(r["restore_reported"] == "rewound", f"{name}-restore-not-reported", f"round {i}: the file restore reported {r['restore_reported']} {r['restore_error'] or ''}")
                    cx.rec.expect(not r["tree_changed_by_failed_restore"], f"{name}-failed-restore-changed-the-tree", f"round {i}: the restore failed ({r['restore_error']}) and the tree is not where it was: " + "; ".join(r["tree_changed_by_failed_restore"]), "arbos-engine tools::git restore — destroy-before-deliver")
                cx.rec.expect(len(window_has) == 4, f"{name}-window-missing-history", f"a fresh window's history lacks {sorted(set(('one', 'first', 'two', 'second')) - set(window_has))}")
                cx.rec.expect(rounds and rounds[0]["files_after"] == ["f1.txt", "f2.txt"], f"{name}-files-not-restored", f"after rewinding to turn 3 the files are {rounds[0]['files_after'] if rounds else None}, expected f1, f2")
            finally:
                for sp in spinners:
                    sp.kill()
                if churn:
                    churn.kill()
        rw.__doc__ = doc
        return rw

    rw_scenario("rw-01-rewind-with-files-keeps-the-history", "Rewind with files: true, no load: the turns before the rewind point stay on the transcript, the file every 50 ms never reads empty, the restore reports, and a fresh window's history shows them.", "none")
    rw_scenario("rw-02-rewind-with-files-keeps-the-history-pinned", "The same rewind with the kernel pinned to one core beside four spinners — the shape in which standing_pass_e2e read the transcript as empty.", "pinned")
    rw_scenario("rw-03-rewind-with-files-keeps-the-history-under-churn", "The same rewind under disk churn (200 MB fsync loops) and a spinner per core.", "churn")
    @reg("rw-08-failed-restore-leaves-the-tree-where-it-was", tags=("rewind", "destructive-order"))
    def rw08(cx):
        """#419's property, probed past its happy path: the checkpoint's work-tree object is made unreadable (one
        loose object removed, a corrupt repository), then rewind with files: true. `git read-tree` fails. The person
        must be told the restore failed, and must still have exactly what they had: the later turn's files, their
        own uncommitted edit, their own untracked note, and HEAD where it was. Anything else is a restore that
        destroyed before it delivered."""
        place = cx.place
        place.mkdir(parents=True, exist_ok=True)
        g = lambda *a: subprocess.run(["git", "-c", "user.name=qa", "-c", "user.email=qa@qa", *a], cwd=place, capture_output=True, text=True)
        for args in (["init", "-q"], ["config", "user.name", "qa"], ["config", "user.email", "qa@qa"], ["config", "gc.auto", "0"], ["commit", "-q", "--allow-empty", "-m", "start"]):
            g(*args)
        (place / ".gitignore").write_text(".arbos/\n")
        g("add", ".gitignore")
        g("commit", "-q", "-m", "ignore .arbos")
        replies = []
        for i, word in enumerate(("first", "second", "third"), 1):
            replies.append({"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": f"echo {word} > f{i}.txt", "description": f"write f{i}"}}]})
            replies.append({"agent": "root", "content": word})
        k = cx.kernel(extra_args=["--provider", "replay", "--replies", str(replies_file(cx, replies))])
        # Every git the kernel runs, with its cwd, exit code and output: the evidence for what a restore did.
        shim = cx.scratch / "git-shim"
        shim.mkdir(exist_ok=True)
        gitlog = cx.rec.dir / "kernel-git.log"
        real_git = shutil.which("git")
        (shim / "git").write_text(f'#!/bin/sh\nout=$(mktemp "{gitlog}.XXXXXX"); err="$out.err"\n"{real_git}" "$@" > "$out" 2> "$err"; rc=$?\ncat "$out"; cat "$err" >&2\n{{ printf "cwd=%s args=" "$PWD"; printf "%s " "$@"; echo; echo "rc=$rc"; sed "s/^/  | /" "$out"; sed "s/^/  ! /" "$err"; }} >> "{gitlog}"; rm -f "$out" "$err"; exit $rc\n')
        (shim / "git").chmod(0o755)
        k.env["PATH"] = f"{shim}:{k.env.get('PATH', '')}"
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        c = k.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        for t in ("one", "two", "three"):
            c.user("root", t)
            cx.rec.expect(c.wait_turn("root", "idle", 60) is not None, "turn-never-ended", f"turn {t!r} never ended")
        # The person's own work after turn three: a commit, an edit, a note nobody else knows about.
        g("add", "f2.txt")
        g("commit", "-q", "-m", "keep f2")
        (place / "f1.txt").write_text("first, edited by hand\n")
        (place / "my-notes.txt").write_text("do not lose this\n")
        before = tree_state(place)
        cx.rec.notes["files_before"] = sorted(before["files"])
        cx.rec.notes["index_before"] = before["index"].splitlines()
        cx.rec.notes["status_before"] = g("status", "--porcelain", "--ignored").stdout.splitlines()
        cx.rec.notes["clean_dry_run_by_harness"] = g("clean", "-fdn", "-e", ".arbos", "-e", ".arbos/**").stdout.splitlines()
        cx.rec.notes["excludes"] = (place / ".git" / "info" / "exclude").read_text().splitlines()[-5:] if (place / ".git" / "info" / "exclude").exists() else None
        # Break the checkpoint's record of the tree: the work commit for turn 3 loses its loose object.
        cps = place / ".arbos" / "agents" / "root" / "checkpoints.jsonl"
        # The tree half of a checkpoint is saved on the blocking pool after the turn starts: wait for turn 3's to land.
        recs, deadline = [], time.time() + 15
        while time.time() < deadline:
            recs = [json.loads(l) for l in cps.read_text().splitlines() if l.strip()] if cps.exists() else []
            if len(recs) >= 3 and (recs[2].get("work") or recs[2].get("clean") or (recs[2].get("work_error") and "pending" not in str(recs[2].get("work_error")).lower() and "being saved" not in str(recs[2].get("work_error")).lower())):
                break
            time.sleep(0.2)
        cx.rec.notes["checkpoints"] = [{k_: (str(v_)[:60]) for k_, v_ in r.items()} for r in recs]
        # Turn 1's checkpoint saw a clean tree (no work commit); turns 2 and 3 have one. Turn 3's is the target.
        target = recs[2] if len(recs) >= 3 else {}
        cx.rec.expect(bool(target.get("work")), "no-work-checkpoint", f"expected a work tree on turn 3's checkpoint, got {[r.get('work') for r in recs]}")
        if not target.get("work"):
            k.stop()
            return
        work = target["work"]
        obj = place / ".git" / "objects" / work[:2] / work[2:]
        cx.rec.notes["work_commit"] = work
        cx.rec.notes["work_object_loose"] = obj.exists()
        if obj.exists():
            obj.unlink()
        else:
            # packed: unpack is not worth it here; corrupt by pointing the ref at a missing object instead
            g("update-ref", "-d", f"refs/arbos/cp/root/{target.get('line', 0)}")
        probe = g("cat-file", "-t", work)
        cx.rec.notes["work_object_readable_after"] = probe.returncode == 0
        cx.rec.expect(probe.returncode != 0, "object-still-readable", f"could not make the work commit unreadable: {probe.stdout.strip()}")
        # Rewind to turn 3 with files: the restore must fail, and fail cleanly.
        c.send({"type": "rewind", "agent": "root", "turn": 3, "files": True})
        first = c.wait(lambda f: f.get("type") == "rewound" and f.get("agent") == "root", 15, "the rewound frame")
        follow = c.wait(lambda f: (f.get("type") == "rewound" and f.get("restored") is not None) or f.get("type") == "error", 30, "the restore's report")
        time.sleep(1.0)
        after = tree_state(place)
        diff = tree_diff(before, after)
        # The pass must not come from git's content addressing: a safety copy of the current tree writes blobs
        # by content, and a missing *blob* could come back that way and make a restore look real (the author of
        # #419 hit this). We remove the work *commit*, whose bytes nothing else in the repository holds; if it is
        # readable again afterwards, something recreated it and the failure was never exercised.
        cx.rec.notes["work_object_readable_after_rewind"] = g("cat-file", "-t", work).returncode == 0
        cx.rec.expect(g("cat-file", "-t", work).returncode != 0, "probe-object-came-back", f"the work commit {work[:10]} is readable again after the rewind: the missing-object failure was not exercised")
        cx.rec.notes["rewound_frame"] = first
        cx.rec.notes["restore_report"] = follow
        cx.rec.notes["tree_diff_after_failed_restore"] = diff
        reported = json.dumps(follow or {})
        said_failed = bool(follow) and (follow.get("type") == "error" or "fail" in reported.lower() or "could not" in reported.lower())
        said_restored = bool(follow) and follow.get("type") == "rewound" and follow.get("restored") and not said_failed
        cx.rec.expect(said_failed, "failed-restore-not-reported", f"read-tree of a missing object must be reported as a failed restore; the client got {reported[:300]}", "arbos-engine tools::git restore / arbos-kernel serve rewind")
        cx.rec.expect(not said_restored, "failed-restore-called-restored", f"the restore could not have happened (work commit {work[:10]} is unreadable) yet the client was told restored: {reported[:300]}")
        cx.rec.expect(not diff, "failed-restore-changed-the-tree", "a restore that failed left the person somewhere new: " + "; ".join(diff), "arbos-engine tools::git restore — a destructive step (reset --hard, clean) runs before the step that can fail (read-tree); check the objects first, or take them back")
        evs, bad = transcript(cx.place, "root")
        cx.rec.expect(not bad, "transcript-corrupt", f"bad lines: {bad}")
        k.stop()
        cx.check()

    def rw08_late_failure(name, doc, sabotage):
      @reg(name, tags=("rewind", "destructive-order"))
      def rw08x(cx):
          place = cx.place
          place.mkdir(parents=True, exist_ok=True)
          g = lambda *a: subprocess.run(["git", "-c", "user.name=qa", "-c", "user.email=qa@qa", *a], cwd=place, capture_output=True, text=True)
          for args in (["init", "-q"], ["config", "user.name", "qa"], ["config", "user.email", "qa@qa"], ["commit", "-q", "--allow-empty", "-m", "start"]):
              g(*args)
          (place / ".gitignore").write_text(".arbos/\n")
          g("add", ".gitignore")
          g("commit", "-q", "-m", "ignore .arbos")
          replies = []
          for i, word in enumerate(("first", "second", "third"), 1):
              replies.append({"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": f"echo {word} > f{i}.txt", "description": f"write f{i}"}}]})
              replies.append({"agent": "root", "content": word})
          k = cx.kernel(extra_args=["--provider", "replay", "--replies", str(replies_file(cx, replies))])
          shim = cx.scratch / "git-shim"
          shim.mkdir(exist_ok=True)
          gitlog = cx.rec.dir / "kernel-git.log"
          real_git = shutil.which("git")
          (shim / "git").write_text(f'#!/bin/sh\nout=$(mktemp "{gitlog}.XXXXXX"); err="$out.err"\n"{real_git}" "$@" > "$out" 2> "$err"; rc=$?\ncat "$out"; cat "$err" >&2\n{{ printf "cwd=%s args=" "$PWD"; printf "%s " "$@"; echo; echo "rc=$rc"; sed "s/^/  | /" "$out"; sed "s/^/  ! /" "$err"; }} >> "{gitlog}"; rm -f "$out" "$err"; exit $rc\n')
          (shim / "git").chmod(0o755)
          k.env["PATH"] = f"{shim}:{k.env.get('PATH', '')}"
          cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
          c = k.attach()
          c.wait(lambda f: f.get("type") == "snapshot", 5)
          for t in ("one", "two", "three"):
              c.user("root", t)
              cx.rec.expect(c.wait_turn("root", "idle", 60) is not None, "turn-never-ended", f"turn {t!r} never ended")
          g("add", "f2.txt")
          g("commit", "-q", "-m", "keep f2")
          (place / "f1.txt").write_text("first, edited by hand\n")
          (place / "my-notes.txt").write_text("do not lose this\n")
          undo = sabotage(place)
          before = tree_state(place)
          try:
              c.send({"type": "rewind", "agent": "root", "turn": 3, "files": True})
              first = c.wait(lambda f: f.get("type") == "rewound" and f.get("agent") == "root", 15, "the rewound frame")
              follow = c.wait(lambda f: (f.get("type") == "rewound" and f.get("restored") is not None) or f.get("type") == "error", 45, "the restore's report")
              time.sleep(1.0)
              after = tree_state(place)
          finally:
              undo()
          diff = tree_diff(before, after)
          cx.rec.notes["restore_report"] = follow
          cx.rec.notes["tree_diff_after_failed_restore"] = diff
          reported = json.dumps(follow or {})
          said_failed = bool(follow) and (follow.get("type") == "error" or "fail" in reported.lower() or "refus" in reported.lower())
          cx.rec.expect(said_failed, "failed-restore-not-reported", f"git could not complete the restore, yet the client got {reported[:300]}")
          cx.rec.expect(not diff, "failed-restore-changed-the-tree", "a restore that failed after its checks left the person somewhere new: " + "; ".join(diff), "arbos-engine tools::git restore — put the tree back on any failure (#419 at 0bceb0df)")
          if said_failed and not diff and follow:
              cx.rec.notes["put_back_said"] = "put back" in reported.lower() or "restored to" in reported.lower()
          evs, bad = transcript(cx.place, "root")
          cx.rec.expect(not bad, "transcript-corrupt", f"bad lines: {bad}")
          k.stop()
          cx.check()
      rw08x.__doc__ = doc
      return rw08x


    def sab_index_lock(place):
        lock = place / ".git" / "index.lock"
        lock.write_text("")
        return lambda: lock.unlink(missing_ok=True)

    def sab_dir_in_the_way(place):
        # The checkpoint has the file f1.txt; the person since replaced it with a folder git cannot empty.
        (place / "f1.txt").unlink()
        d = place / "f1.txt"
        d.mkdir()
        (d / "inner.txt").write_text("in the way\n")
        d.chmod(0o555)
        return lambda: d.chmod(0o755)

    rw08_late_failure("rw-08b-restore-that-fails-before-anything-moves-leaves-the-tree", "`.git/index.lock` exists (a crashed git, another git running): `reset --hard` fails after the objects were verified and before anything moved. Same tree afterwards, told plainly.", sab_index_lock)
    rw08_late_failure("rw-08c-restore-that-fails-half-way-puts-the-tree-back", "The checkpoint has the file f1.txt; the person replaced it with a folder git cannot empty (no write bit, a file inside). Objects verify, `reset --hard` succeeds, `read-tree -u` then fails: the person must be told and must have exactly what they had — on #419 at 0bceb0df by the tree being put back.", sab_dir_in_the_way)

    @reg("rw-10-rewind-while-another-git-commits-in-the-same-repository", tags=("rewind", "concurrency"))
    def rw10(cx):
        """Jacob's normal state: a terminal open in the project, git committing there while the kernel works. A second
        git renames index.lock over index, so for an instant .git/index is not a regular file; the checkpoint's index
        copy must survive that (retry, #419 at 5340c0d2), every turn must still get a work tree, and a rewind with files
        landing in the middle of that churn must either restore fully or fail with the tree where it was."""
        place = cx.place
        place.mkdir(parents=True, exist_ok=True)
        g = lambda *a: subprocess.run(["git", "-c", "user.name=qa", "-c", "user.email=qa@qa", *a], cwd=place, capture_output=True, text=True)
        for args in (["init", "-q"], ["config", "user.name", "qa"], ["config", "user.email", "qa@qa"], ["config", "gc.auto", "0"], ["commit", "-q", "--allow-empty", "-m", "start"]):
            g(*args)
        (place / ".gitignore").write_text(".arbos/\n")
        g("add", ".gitignore")
        g("commit", "-q", "-m", "ignore .arbos")
        replies = []
        for i, word in enumerate(("first", "second", "third", "fourth", "fifth", "sixth"), 1):
            replies.append({"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": f"echo {word} > f{i}.txt", "description": f"write f{i}"}}]})
            replies.append({"agent": "root", "content": word})
        # The other git: a tight loop of `git add`/`git commit` on its own file, each one an index.lock → index rename.
        churn_log = cx.rec.dir / "other-git.log"
        # Several terminals, not one: the window (index.lock renamed over index) is a few microseconds wide and the
        # kernel copies the index six times; one loop at ~100 commits/s rarely meets it. Three loops on one repo
        # also contend with each other on index.lock, which is what a busy repository looks like.
        n_churn = int(os.environ.get("ARBOS_QA_RW10_CHURNERS", "3"))
        churners = [subprocess.Popen(["sh", "-c", f'cd "{place}" && n=0; while :; do n=$((n+1)); echo $n-{i} > terminal-{i}.txt; git -c user.name=t -c user.email=t@t add terminal-{i}.txt >/dev/null 2>&1; git -c user.name=t -c user.email=t@t commit -q -m "terminal {i} $n" >/dev/null 2>&1; done'], stdout=open(churn_log, "ab"), stderr=subprocess.STDOUT, env={**os.environ, "GIT_CONFIG_GLOBAL": "/dev/null"}) for i in range(n_churn)]
        class _Churn:
            def kill(self):
                for p_ in churners:
                    p_.kill()
            def wait(self, timeout=None):
                for p_ in churners:
                    p_.wait(timeout=timeout)
            def poll(self):
                return None if any(p_.poll() is None for p_ in churners) else 0
        churn = _Churn()
        k = cx.kernel(extra_args=["--provider", "replay", "--replies", str(replies_file(cx, replies))])
        try:
            cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
            c = k.attach()
            c.wait(lambda f: f.get("type") == "snapshot", 5)
            for t in ("one", "two", "three", "four", "five", "six"):
                c.user("root", t)
                cx.rec.expect(c.wait_turn("root", "idle", 60) is not None, "turn-never-ended", f"turn {t!r} never ended")
            cps = place / ".arbos" / "agents" / "root" / "checkpoints.jsonl"
            recs, deadline = [], time.time() + 20
            while time.time() < deadline:
                recs = [json.loads(l) for l in cps.read_text().splitlines() if l.strip()] if cps.exists() else []
                pending = [r for r in recs if not (r.get("work") or r.get("clean")) and ("pending" in str(r.get("work_error", "")).lower() or "being saved" in str(r.get("work_error", "")).lower())]
                if len(recs) >= 6 and not pending:
                    break
                time.sleep(0.2)
            errors = [r.get("work_error") for r in recs if r.get("work_error") and not (r.get("work") or r.get("clean"))]
            cx.rec.notes["checkpoints"] = [{"line": r.get("line"), "work": (r.get("work") or "")[:10], "clean": r.get("clean"), "work_error": (r.get("work_error") or "")[:120]} for r in recs]
            cx.rec.notes["other_git_commits_during_turns"] = int(g("rev-list", "--count", "HEAD").stdout.strip() or 0)
            cx.rec.expect(len(recs) >= 6, "checkpoints-missing", f"six turns, {len(recs)} checkpoint record(s)")
            cx.rec.expect(not errors, "checkpoint-tree-lost-to-the-other-git", f"{len(errors)} of {len(recs)} checkpoints have no work tree while another git was committing: {errors[:3]}", "arbos-engine tools::git snapshot_turn_tree — copy the index while another git renames it (#419 at 5340c0d2 retries)")
            # Now the rewind, with the other git still going.
            before = tree_state(place)
            c.send({"type": "rewind", "agent": "root", "turn": 4, "files": True})
            first = c.wait(lambda f: f.get("type") == "rewound" and f.get("agent") == "root", 15, "the rewound frame")
            follow = c.wait(lambda f: (f.get("type") == "rewound" and f.get("restored") is not None) or f.get("type") == "error", 45, "the restore's report")
            churn.kill()
            churn.wait(timeout=5)
            time.sleep(0.5)
            after = tree_state(place)
            cx.rec.notes["restore_report"] = follow
            failed = bool(follow) and follow.get("type") == "error"
            files = sorted(p_.name for p_ in place.glob("f*.txt"))
            cx.rec.notes["files_after"] = files
            cx.rec.expect(follow is not None, "restore-never-reported", "no second rewound frame and no error within 45 s")
            if failed:
                # terminal.txt keeps changing under the other git until it is killed; judge everything else.
                diff = [d for d in tree_diff(before, after) if not d.startswith("terminal-") and not d.startswith("HEAD") and not d.startswith("index")]
                cx.rec.notes["tree_diff_after_failed_restore"] = diff
                cx.rec.expect(not diff, "failed-restore-changed-the-tree", "the restore failed under another git and the tree is not where it was: " + "; ".join(diff))
            else:
                cx.rec.expect(files == ["f1.txt", "f2.txt", "f3.txt"], "files-not-restored", f"after rewinding to turn 4 the files are {files}, expected f1..f3 (kernel's own), whatever the terminals' gits did to terminal-*.txt")
        finally:
            if churn.poll() is None:
                churn.kill()
        evs, bad = transcript(cx.place, "root")
        cx.rec.expect(not bad, "transcript-corrupt", f"bad lines: {bad}")
        k.stop()
        cx.check()

    @reg("rw-10b-index-is-not-a-regular-file-for-an-instant-when-the-checkpoint-copies-it", tags=("rewind", "concurrency"))
    def rw10b(cx):
        """The instant rw-10's churn rarely lands on, made certain: as each turn's checkpoint record appears, .git/index
        is swapped for a directory for 120 ms and put back — to a copy, the same "not a regular file" that another git's
        index.lock → index rename shows for a moment, held long enough to be sure. (Not a FIFO: the first version used
        one, and an open() on a FIFO with no writer blocks forever — the kernel's turn hung for good on every build, a
        failure the harness had invented.) The checkpoint's index copy must ride it out (#419 at 5340c0d2 retries for a
        quarter second) and every turn must end with a work tree."""
        place = cx.place
        place.mkdir(parents=True, exist_ok=True)
        g = lambda *a: subprocess.run(["git", "-c", "user.name=qa", "-c", "user.email=qa@qa", *a], cwd=place, capture_output=True, text=True)
        for args in (["init", "-q"], ["config", "user.name", "qa"], ["config", "user.email", "qa@qa"], ["commit", "-q", "--allow-empty", "-m", "start"]):
            g(*args)
        (place / ".gitignore").write_text(".arbos/\n")
        g("add", ".gitignore")
        g("commit", "-q", "-m", "ignore .arbos")
        replies = []
        for i, word in enumerate(("first", "second", "third"), 1):
            replies.append({"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": f"echo {word} > f{i}.txt", "description": f"write f{i}"}}]})
            replies.append({"agent": "root", "content": word})
        cps = place / ".arbos" / "agents" / "root" / "checkpoints.jsonl"
        index = place / ".git" / "index"
        swaps = []
        stop = threading.Event()

        def swapper():
            seen = 0
            while not stop.is_set():
                try:
                    n = sum(1 for l in cps.read_text().splitlines() if l.strip()) if cps.exists() else 0
                except OSError:
                    n = seen
                if n > seen:
                    seen = n
                    if os.environ.get("ARBOS_QA_RW10B_NO_SWAP") == "1":
                        swaps.append(0)  # control: measure the base race with nothing in the way
                    elif index.exists():
                        real = index.with_name("index.real-for-a-moment")
                        try:
                            index.rename(real)
                            index.mkdir()
                            t0 = time.time()
                            time.sleep(0.12)
                            index.rmdir()
                            real.rename(index)
                            swaps.append(round((time.time() - t0) * 1000))
                        except OSError as e:
                            swaps.append(f"swap failed: {e}")
                            if real.exists() and not index.exists():
                                real.rename(index)
                time.sleep(0.002)

        th = threading.Thread(target=swapper, daemon=True)
        th.start()
        k = cx.kernel(extra_args=["--provider", "replay", "--replies", str(replies_file(cx, replies))])
        try:
            cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
            c = k.attach()
            c.wait(lambda f: f.get("type") == "snapshot", 5)
            for t in ("one", "two", "three"):
                c.user("root", t)
                cx.rec.expect(c.wait_turn("root", "idle", 60) is not None, "turn-never-ended", f"turn {t!r} never ended")
            recs, deadline = [], time.time() + 20
            while time.time() < deadline:
                recs = [json.loads(l) for l in cps.read_text().splitlines() if l.strip()] if cps.exists() else []
                pending = [r for r in recs if not (r.get("work") or r.get("clean")) and ("pending" in str(r.get("work_error", "")).lower() or "being saved" in str(r.get("work_error", "")).lower())]
                if len(recs) >= 3 and not pending:
                    break
                time.sleep(0.2)
        finally:
            stop.set()
            th.join(timeout=2)
            if not index.exists() and index.with_name("index.real-for-a-moment").exists():
                index.with_name("index.real-for-a-moment").rename(index)
        errors = [r.get("work_error") for r in recs if r.get("work_error") and not (r.get("work") or r.get("clean"))]
        # A checkpoint is the tree *before* its turn. Turn N writes fN.txt; checkpoint N's tree must not hold it.
        # The tree is taken on the blocking pool while the turn runs, so a delay (a retrying index copy, a slow
        # disk) can let the turn's first write into "before".
        late = []
        for i, r in enumerate(recs[:3], 1):
            if r.get("work"):
                names = g("ls-tree", "-r", "--name-only", r["work"]).stdout.split()
                if f"f{i}.txt" in names:
                    late.append(f"checkpoint {i} ({r['work'][:10]}) already holds f{i}.txt, which turn {i} wrote")
        cx.rec.notes["checkpoint_after_turn_wrote"] = late
        cx.rec.expect(not late, "checkpoint-taken-after-the-turn-wrote", "; ".join(late), "arbos-engine turn.rs — the tree snapshot runs beside the turn; the turn's first tool call can land first")
        cx.rec.notes["swaps_ms"] = swaps
        cx.rec.notes["checkpoints"] = [{"line": r.get("line"), "work": (r.get("work") or "")[:10], "clean": r.get("clean"), "work_error": (r.get("work_error") or "")[:120]} for r in recs]
        cx.rec.expect(len(swaps) >= 3 and all(isinstance(x, int) for x in swaps), "probe-did-not-swap", f"the index was not swapped for every checkpoint: {swaps} — this run proves nothing")
        cx.rec.expect(not errors, "checkpoint-tree-lost-to-a-momentary-index", f"{len(errors)} of {len(recs)} checkpoints have no work tree after a 120 ms instant in which .git/index was not a regular file: {errors[:3]}", "arbos-engine tools::git snapshot_turn_tree — copy the index (#419 at 5340c0d2 retries 10 × 25 ms)")
        evs, bad = transcript(cx.place, "root")
        cx.rec.expect(not bad, "transcript-corrupt", f"bad lines: {bad}")
        k.stop()
        cx.check()

    @reg("rw-10c-a-long-wait-for-the-checkpoint-tree-is-shown-as-the-command-running", tags=("rewind", "misreport"))
    def rw10c(cx):
        """#419 at 2daa555d: a tool that writes waits for the turn's checkpoint tree. On a large repository that wait is
        seconds (here: ARBOS_TEST_TREE_DELAY_MS=6000, the kernel's own knob for a slow `add -A`). New behaviour, so the
        question is what a person sees during it. Measured: not the silent stall (a status and a running tool card
        appear at once) but the wrong story — status `Running echo first > f1.txt` and a running card for six seconds,
        and a tool record that says the command ran 6.3 s. The wait must be named as its own step and not charged to
        the tool."""
        place = cx.place
        place.mkdir(parents=True, exist_ok=True)
        g = lambda *a: subprocess.run(["git", "-c", "user.name=qa", "-c", "user.email=qa@qa", *a], cwd=place, capture_output=True, text=True)
        for args in (["init", "-q"], ["config", "user.name", "qa"], ["config", "user.email", "qa@qa"], ["commit", "-q", "--allow-empty", "-m", "start"]):
            g(*args)
        (place / ".gitignore").write_text(".arbos/\n")
        g("add", ".gitignore")
        g("commit", "-q", "-m", "ignore .arbos")
        replies = [
            {"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": "echo first > f1.txt", "description": "write f1"}}]},
            {"agent": "root", "content": "first"},
        ]
        k = cx.kernel(extra_args=["--provider", "replay", "--replies", str(replies_file(cx, replies))])
        k.env["ARBOS_TEST_TREE_DELAY_MS"] = "6000"
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        c = k.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        t0_ms = now_ms()
        c.user("root", "one")
        cx.rec.expect(c.wait_turn("root", "idle", 60) is not None, "turn-never-ended", "the turn never ended")
        time.sleep(0.3)
        # What reached the window after the person's message, in order, with the delay from it.
        timeline = []
        for at, f in list(c.frames):
            if at < t0_ms:
                continue
            kind = f.get("type") + ("/" + str((f.get("event") or {}).get("kind")) if f.get("type") == "event" else "")
            timeline.append((round((at - t0_ms) / 1000, 2), kind, json.dumps(f)[:160]))
        first_visible = next((t for t, kind, _ in timeline if kind not in ("tree", "plan")), None)
        named = [blob for _, _, blob in timeline if any(w in blob.lower() for w in ("checkpoint", "waiting", "snapshot"))]
        evs, bad = transcript(cx.place, "root")
        tool = next((e for e in evs if e.get("kind") == "tool"), {})
        user_ts = next((e.get("ts") for e in evs if e.get("kind") == "user"), None)
        tool_span_s = round(((tool.get("ended") or 0) - (tool.get("started") or 0)) / 1000, 1) if tool else None
        wait_s = round(((tool.get("started") or 0) - (user_ts or 0)) / 1000, 1) if tool and user_ts else None
        notices = [e.get("text", "")[:160] for e in evs if e.get("kind") == "notice"]
        stderr_line = ""
        try:
            for l in (cx.rec.dir / "kernel.stderr.log").read_text(errors="replace").splitlines():
                if "waited" in l and "checkpoint" in l:
                    stderr_line = l[:160]
        except OSError:
            pass
        cx.rec.notes.update({"timeline_after_message": timeline[:12], "seconds_to_first_visible_frame": first_visible, "frames_naming_the_wait": named[:5], "notices": notices, "tool_record": {"started_after_message_s": wait_s, "started_to_ended_s": tool_span_s, "label": tool.get("label")}, "kernel_stderr_waited": stderr_line})
        waited_somewhere = bool(stderr_line) or (wait_s is not None and wait_s >= 4.0) or (tool_span_s is not None and tool_span_s >= 4.0)
        cx.rec.notes["wait_logged_on_stderr"] = bool(stderr_line)
        cx.rec.expect(waited_somewhere, "probe-did-not-wait", "neither the tool record nor the kernel's stderr shows a wait; the delay knob did not hold the tool, this run proves nothing")
        cx.rec.expect(first_visible is not None and first_visible < 2.0, "silent-wait", f"{first_visible}s after the person's message before anything but a tree/plan frame reached the window; the kernel waited for the checkpoint tree ({stderr_line.split(': ', 1)[-1] if stderr_line else '?'}) and nothing the window draws said so — the silent stall shape (st-01) with correct data underneath", "arbos-engine turn.rs — the checkpoint wait (#419 at 2daa555d) is logged to stderr only")
        status_steps = [json.loads(b).get("step") for _, k_, b in timeline if k_ == "status" and json.loads(b).get("step")]
        cx.rec.notes["status_steps_shown"] = status_steps
        # The fix (#419 at a5072074): the wait is the kernel's own step, said first; the command's step follows.
        ck = next((i for i, st in enumerate(status_steps) if "checkpoint" in st.lower()), None)
        run_ = next((i for i, st in enumerate(status_steps) if st.lower().startswith("running")), None)
        cx.rec.expect(ck is not None and (run_ is None or ck < run_), "wait-not-named-first", f"the status steps shown were {status_steps}: no `Saving a checkpoint…` step before the command's own", "arbos-engine batch.rs kernel_step (#419 at a5072074)")
        cx.rec.expect(tool_span_s is None or tool_span_s < 2.0, "wait-charged-to-the-tool", f"the window showed {status_steps[:1]} and a running tool card for the whole wait, and the transcript's tool record says `{tool.get('label')}` ran for {tool_span_s}s (started {wait_s}s after the message): the checkpoint wait is shown and recorded as the command's own running time — a person sees `echo` hang for six seconds, and history and any duration view blame it", "arbos-engine turn.rs — the checkpoint wait (#419 at 2daa555d) happens inside the tool's started..ended and under the tool's status; name it as its own step")
        cx.rec.expect((place / "f1.txt").exists(), "tool-did-not-run", "f1.txt was never written")
        cx.rec.expect(not bad, "transcript-corrupt", f"bad lines: {bad}")
        k.stop()
        cx.check()

    @reg("rw-10d-the-checkpoint-step-never-overwrites-a-status-the-agent-set", tags=("rewind", "misreport"))
    def rw10d(cx):
        """qal-j18's fix (#419 at a5072074) shows `Saving a checkpoint of the working tree` as a derived status. A status
        the agent set itself this turn (the `status` tool) must stay: the derived line is a guess, the agent's words are
        not. Same six-second tree delay; the reply sets a status first, then writes."""
        place = cx.place
        place.mkdir(parents=True, exist_ok=True)
        g = lambda *a: subprocess.run(["git", "-c", "user.name=qa", "-c", "user.email=qa@qa", *a], cwd=place, capture_output=True, text=True)
        for args in (["init", "-q"], ["config", "user.name", "qa"], ["config", "user.email", "qa@qa"], ["commit", "-q", "--allow-empty", "-m", "start"]):
            g(*args)
        (place / ".gitignore").write_text(".arbos/\n")
        g("add", ".gitignore")
        g("commit", "-q", "-m", "ignore .arbos")
        replies = [
            {"agent": "root", "content": "", "calls": [{"name": "status", "arguments": {"step": "Sorting the samples"}}, {"name": "bash", "arguments": {"command": "echo first > f1.txt", "description": "write f1"}}]},
            {"agent": "root", "content": "first"},
        ]
        k = cx.kernel(extra_args=["--provider", "replay", "--replies", str(replies_file(cx, replies))])
        k.env["ARBOS_TEST_TREE_DELAY_MS"] = "6000"
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        c = k.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        t0_ms = now_ms()
        c.user("root", "one")
        cx.rec.expect(c.wait_turn("root", "idle", 60) is not None, "turn-never-ended", "the turn never ended")
        time.sleep(0.3)
        statuses = [(round((at - t0_ms) / 1000, 2), f.get("step"), f.get("source")) for at, f in list(c.frames) if at >= t0_ms and f.get("type") == "status" and f.get("agent") == "root"]
        cx.rec.notes["statuses"] = statuses
        agent_at = next((t for t, step, src in statuses if src == "agent" and step == "Sorting the samples"), None)
        derived_after = [(t, step) for t, step, src in statuses if src == "derived" and step and agent_at is not None and t >= agent_at]
        stderr = ""
        try:
            stderr = next((l for l in (cx.rec.dir / "kernel.stderr.log").read_text(errors="replace").splitlines() if "waited" in l and "checkpoint" in l), "")
        except OSError:
            pass
        evs_, _ = transcript(place, "root")
        tool_ = next((e for e in evs_ if e.get("kind") == "tool" and e.get("name") == "bash"), {})
        user_ts_ = next((e.get("ts") for e in evs_ if e.get("kind") == "user"), None)
        held = bool(stderr) or (tool_ and user_ts_ and ((tool_.get("ended") or 0) - user_ts_) >= 4000)
        cx.rec.expect(held, "probe-did-not-wait", "the kernel did not wait for the tree (tool ended under 4 s after the message, nothing on stderr); this run proves nothing")
        cx.rec.expect(agent_at is not None, "agent-status-not-shown", f"the agent's own status never reached the window: {statuses}")
        cx.rec.expect(not derived_after, "derived-status-overwrote-the-agents", f"a derived status replaced the agent's `Sorting the samples` during the turn: {derived_after}", "arbos-kernel hooks.rs set_status — a guess never overwrites what the agent said this turn")
        cx.rec.expect((place / "f1.txt").exists(), "tool-did-not-run", "f1.txt was never written")
        k.stop()
        cx.check()

    # ── #432: the coordinator that slept on its workers ───────────────────────
    def co_setup(cx):
        place = cx.place
        place.mkdir(parents=True, exist_ok=True)
        return place

    def co_run(cx, replies, prompt, tag="kernel", timeout=90):
        k = cx.kernel(tag=tag, extra_args=["--provider", "replay", "--replies", str(replies_file(cx, replies))])
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        c = k.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        t0 = time.time()
        c.user("root", prompt)
        ended = c.wait_turn("root", "idle", timeout)
        return k, c, t0, ended

    def bash_records(place, agent="root"):
        evs, _ = transcript(place, agent)
        return [e for e in evs if e.get("kind") == "tool" and e.get("name") == "bash"]

    @reg("co-01-bare-sleep-with-workers-is-refused-and-the-report-arrives", tags=("coordinator", "sleep"))
    def co01(cx):
        """Jacob's report: three workers spawned, then `sleep 75`, reports queued behind the sleeping turn. #432: a bare
        sleep of 5 s or more while the agent has workers is refused, naming how many and the move; the worker's report
        then starts the next turn."""
        place = co_setup(cx)
        replies = [
            {"agent": "root", "content": "", "calls": [{"name": "spawn", "arguments": {"name": "sorter", "task": "Sort the samples and report"}}]},
            {"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": "sleep 75; echo waited", "description": "wait for the worker"}}]},
            {"agent": "root", "content": "Waiting on the sorter."},
            {"content": "", "calls": [{"name": "bash", "arguments": {"command": "sleep 6; echo sorted", "description": "sort"}}]},
            {"content": "Sorted: a b c."},
            {"agent": "root", "content": "The sorter reports: a b c."},
        ]
        k, c, t0, ended = co_run(cx, replies, "Spawn a sorter and wait for it.", timeout=60)
        first_turn_s = round(time.time() - t0, 1)
        recs = bash_records(place)
        sleep_rec = next((r for r in recs if "sleep 75" in json.dumps(r.get("args") or {})), None)
        err = str((sleep_rec or {}).get("error") or "")
        cx.rec.notes["sleep_record"] = {k_: str(v)[:200] for k_, v in (sleep_rec or {}).items() if k_ in ("error", "output", "started", "ended")}
        cx.rec.notes["first_turn_s"] = first_turn_s
        cx.rec.expect(ended is not None, "turn-never-ended", "root's first turn never ended")
        cx.rec.expect(sleep_rec is not None, "sleep-not-recorded", "no bash record for the sleep")
        cx.rec.expect("refus" in err.lower() and "worker" in err.lower(), "sleep-ran-with-workers", f"`sleep 75` with a worker running was not refused: error={err[:160]!r} output={str((sleep_rec or {}).get('output'))[:80]!r}", "arbos-engine tools/bash.rs bare_sleep_secs / children_count (#432)")
        cx.rec.expect(first_turn_s < 20, "turn-slept-anyway", f"root's turn took {first_turn_s}s; a refused sleep must not cost the wait")
        cx.rec.expect("end the turn" in err.lower() or "await" in err.lower(), "refusal-names-no-alternative", f"the refusal does not say what to do instead: {err[:200]}")
        # The worker's report must start root's next turn and be answered.
        end = time.time() + 45
        answered = False
        while time.time() < end:
            evs, _ = transcript(place, "root")
            if any(e.get("kind") == "assistant" and "a b c" in (e.get("text") or "") for e in evs):
                answered = True
                break
            time.sleep(0.5)
        cx.rec.expect(answered, "report-never-answered", "the worker finished but root never took a turn on its report within 45 s")
        k.stop()
        cx.check()

    @reg("co-02-short-sleep-and-sleep-without-workers-still-run", tags=("coordinator", "sleep"))
    def co02(cx):
        """The refusal must not be broader than the fault: a 3 s sleep with a worker running, and a 6 s sleep with no
        workers at all, are legitimate and must run to completion with their output."""
        place = co_setup(cx)
        replies = [
            {"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": "sleep 6; echo no-workers-waited", "description": "wait, no workers"}}]},
            {"agent": "root", "content": "", "calls": [{"name": "spawn", "arguments": {"name": "helper", "task": "Help and report"}}]},
            {"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": "sleep 3 && echo short-waited", "description": "short wait"}}]},
            {"agent": "root", "content": "Both waits ran."},
            {"content": "", "calls": [{"name": "bash", "arguments": {"command": "sleep 20; echo helped", "description": "help"}}]},
            {"content": "Helped."},
            {"agent": "root", "content": "The helper reports."},
        ]
        k, c, t0, ended = co_run(cx, replies, "Wait six seconds, spawn a helper, wait three seconds.", timeout=90)
        recs = bash_records(place)
        outs = {json.dumps(r.get("args") or {})[:60]: (str(r.get("output") or ""), str(r.get("error") or "")) for r in recs}
        cx.rec.notes["bash_records"] = outs
        long_no_workers = next(((o, e) for a, (o, e) in outs.items() if "sleep 6" in a), ("", ""))
        short_with = next(((o, e) for a, (o, e) in outs.items() if "sleep 3" in a), ("", ""))
        cx.rec.expect(ended is not None, "turn-never-ended", "root's turn never ended")
        cx.rec.expect("no-workers-waited" in long_no_workers[0], "sleep-without-workers-refused", f"`sleep 6` with no workers did not run to its output: output={long_no_workers[0][:80]!r} error={long_no_workers[1][:160]!r}", "arbos-engine tools/bash.rs — the sleep refusal is broader than 'while the agent has workers' (#432)")
        cx.rec.expect("short-waited" in short_with[0], "short-sleep-with-workers-refused", f"`sleep 3` with a worker running did not run to its output: output={short_with[0][:80]!r} error={short_with[1][:160]!r}", "arbos-engine tools/bash.rs — the sleep refusal catches sleeps under 5 s (#432)")
        k.stop()
        cx.check()

    @reg("co-03-sleep-inside-a-script-runs-and-the-bare-spellings-that-slip-past-are-listed", tags=("coordinator", "sleep"))
    def co03(cx):
        """#432 scopes the refusal to a bare `sleep N` heading the command. A sleep inside a loop is meant to run. The
        spellings that mean the same wait but are not bare — `sh -c 'sleep 75'`, `/bin/sleep 75`, `timeout 80 sleep 75`,
        `true && sleep 75` — are recorded here for what the kernel does with them, as an observation and not a break:
        the PR's contract is the bare form, and this is the list a reviewer should see."""
        place = co_setup(cx)
        spellings = ["for i in 1 2 3; do sleep 1; done; echo looped", "sh -c 'sleep 8'; echo via-sh", "/bin/sleep 8; echo via-path", "timeout 20 sleep 8; echo via-timeout", "true && sleep 8; echo via-and"]
        replies = [{"agent": "root", "content": "", "calls": [{"name": "spawn", "arguments": {"name": "helper", "task": "Help and report"}}]}]
        for s_ in spellings:
            replies.append({"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": s_, "description": "wait"}}]})
        replies += [{"agent": "root", "content": "Done waiting."}, {"content": "", "calls": [{"name": "bash", "arguments": {"command": "sleep 60; echo helped", "description": "help"}}]}, {"content": "Helped."}, {"agent": "root", "content": "The helper reports."}]
        k, c, t0, ended = co_run(cx, replies, "Spawn a helper, then wait in several ways.", timeout=120)
        recs = bash_records(place)
        verdicts = []
        for s_ in spellings:
            r = next((r for r in recs if (r.get("args") or {}).get("command") == s_), {})
            err = str(r.get("error") or "")
            verdicts.append({"command": s_, "refused": "refus" in err.lower(), "ran": bool(r) and not err and "echo" in s_ and s_.split("echo ")[-1].strip() in str(r.get("output") or ""), "error": err[:120]})
        cx.rec.notes["verdicts"] = verdicts
        loop = verdicts[0]
        cx.rec.expect(ended is not None, "turn-never-ended", "root's turn never ended")
        cx.rec.expect(loop["ran"], "sleep-in-a-loop-refused-or-lost", f"the loop with `sleep 1` inside did not run to its output: {loop}", "arbos-engine tools/bash.rs bare_sleep_secs (#432) — a sleep inside a script is meant to run")
        slipped = [v["command"] for v in verdicts[1:] if v["ran"]]
        cx.rec.notes["same-wait-not-bare-and-ran"] = slipped
        k.stop()
        cx.check()

    @reg("co-04-attached-command-yields-to-a-workers-report-and-its-result-still-arrives", tags=("coordinator", "yield"))
    def co04(cx):
        """The yielding path is the one that can lose work. Root runs an attached twelve-second loop (not a bare sleep, which
        would be refused outright) while a worker finishes in ~2 s. #432: the command yields when the worker's report lands, goes on as a job, and
        its result follows. Check both halves: the yield is said, and the command's own output reaches the record."""
        place = co_setup(cx)
        replies = [
            {"agent": "root", "content": "", "calls": [{"name": "spawn", "arguments": {"name": "quick", "task": "Report at once"}}]},
            {"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": "for i in $(seq 12); do sleep 1; done; echo finished-after-yield", "description": "long attached command"}}]},
            {"agent": "root", "content": "Command started."},
            {"content": "", "calls": [{"name": "bash", "arguments": {"command": "sleep 2; echo quick-done", "description": "quick"}}]},
            {"content": "Quick: done."},
            {"agent": "root", "content": "Noted the quick worker."},
            {"agent": "root", "content": "Noted the command's result."},
            {"agent": "root", "content": "Still here."},
        ]
        k, c, t0, ended = co_run(cx, replies, "Spawn a quick worker and run a long command.", timeout=60)
        first_turn_s = round(time.time() - t0, 1)
        recs = bash_records(place)
        long_rec = next((r for r in recs if "finished-after-yield" in json.dumps(r.get("args") or {})), {})
        blob = json.dumps(long_rec)
        yielded = "report landed" in blob.lower() or "follows this result" in blob.lower() or "yield" in blob.lower()
        cx.rec.notes["first_turn_s"] = first_turn_s
        cx.rec.notes["long_command_record"] = {k_: str(v)[:200] for k_, v in long_rec.items() if k_ in ("error", "output", "body", "started", "ended")}
        cx.rec.expect(ended is not None, "turn-never-ended", "root's first turn never ended")
        cx.rec.expect(yielded and first_turn_s < 10, "no-yield-to-the-report", f"the attached command did not yield to the worker's report (turn took {first_turn_s}s; record: {blob[:200]})", "arbos-engine tools/bash.rs — yield on a worker's done (#432)")
        # The command must have gone on as a job (its out.log fills within its own twelve seconds), and its result must
        # then reach root's record somewhere other than the call's own arguments (a wake, a notice, a job record).
        end = time.time() + 25
        job_out = None
        while time.time() < end and job_out is None:
            jobs = place / ".arbos" / "agents" / "root" / "jobs"
            for j in sorted(jobs.glob("*/out.log")) if jobs.exists() else []:
                if "finished-after-yield" in j.read_text(errors="replace"):
                    job_out = str(j.relative_to(place))
            time.sleep(0.5)
        arrived = None
        end = time.time() + 20
        while time.time() < end and arrived is None:
            evs, _ = transcript(place, "root")
            for e in evs:
                if e.get("kind") == "tool" and "finished-after-yield" in json.dumps(e.get("args") or {}) and "finished-after-yield" not in json.dumps({k_: v for k_, v in e.items() if k_ != "args"}):
                    continue  # the call's own record, naming the command
                if "finished-after-yield" in json.dumps(e):
                    arrived = {"kind": e.get("kind"), "text": json.dumps(e)[:200]}
                    break
            time.sleep(0.5)
        cx.rec.notes["job_output_file"] = job_out
        cx.rec.notes["result_reached_root_as"] = arrived
        cx.rec.expect(job_out is not None, "yielded-command-dropped", "the yielded command never finished as a job: no jobs/*/out.log holds its output", "arbos-engine tools/bash.rs — a yielded command must go on as a job (#432)")
        cx.rec.expect(arrived is not None, "yielded-result-never-reported", "the command finished as a job (its out.log holds the output) but its result never reached root's transcript as anything but the call's own arguments within 20 s", "arbos-kernel — job completion → the agent's record (#432)")
        k.stop()
        cx.check()

    @reg("co-05-sleep-after-the-workers-are-done", tags=("coordinator", "sleep"))
    def co05(cx):
        """#432 counts non-archived children as workers. A worker that has finished and reported is no longer anything
        to wait for; a `sleep 6` then is legitimate. Measured twice: right after the report (worker done, perhaps not yet
        archived) and after the archive."""
        place = co_setup(cx)
        replies = [
            {"agent": "root", "content": "", "calls": [{"name": "spawn", "arguments": {"name": "poet", "task": "Write one line and report"}}]},
            {"agent": "root", "content": "spawned"},
            {"content": "Rain writes on the roof."},
            {"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": "sleep 6; echo after-report", "description": "wait after the report"}}]},
            {"agent": "root", "content": "waited after the report"},
            {"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": "sleep 6; echo after-archive", "description": "wait after the archive"}}]},
            {"agent": "root", "content": "waited after the archive"},
        ]
        k, c, t0, ended = co_run(cx, replies, "Spawn a poet.", timeout=60)
        # The report starts root's second turn, which sleeps.
        end = time.time() + 60
        while time.time() < end:
            recs = bash_records(place)
            if any("after-report" in json.dumps(r.get("args") or {}) and (r.get("ended") or r.get("error")) for r in recs):
                break
            time.sleep(0.5)
        recs = bash_records(place)
        r1 = next((r for r in recs if "after-report" in json.dumps(r.get("args") or {})), {})
        archived_wait_end = time.time() + 60
        while time.time() < archived_wait_end and not (place / ".arbos" / "archive" / "agents" / "poet").exists():
            time.sleep(1)
        archived = (place / ".arbos" / "archive" / "agents" / "poet").exists()
        c.user("root", "Wait again.")
        c.wait_turn("root", "idle", 60)
        time.sleep(0.5)
        recs = bash_records(place)
        r2 = next((r for r in recs if "after-archive" in json.dumps(r.get("args") or {})), {})
        cx.rec.notes.update({"after_report": {k_: str(v)[:160] for k_, v in r1.items() if k_ in ("error", "output")}, "archived_before_second_wait": archived, "after_archive": {k_: str(v)[:160] for k_, v in r2.items() if k_ in ("error", "output")}})
        cx.rec.expect("after-report" in str(r1.get("output") or ""), "sleep-refused-after-the-report", f"the worker had reported and was done; `sleep 6` was still refused: {str(r1.get('error') or '')[:200]}", "arbos-engine tools/bash.rs children_count — a finished, unarchived worker still counts (#432)")
        cx.rec.expect(not archived or "after-archive" in str(r2.get("output") or ""), "sleep-refused-after-the-archive", f"the worker was archived; `sleep 6` was still refused: {str(r2.get('error') or '')[:200]}")
        k.stop()
        cx.check()

    # ── #441: a held place is said once, then escalates ─────────────────────────
    def relaunch(cx, place, env=None, timeout=20):
        """One supervisor relaunch: the kernel binary against a held place, as the supervisor would run it. Returns
        (exit code, stderr)."""
        wrap = ["bash", os.environ["ARBOS_QA_NS_WRAP"]] if os.environ.get("ARBOS_QA_NS_WRAP") and os.environ.get("ARBOS_QA_STORE_VISIBLE") != "1" else []
        p_ = subprocess.run([*wrap, cx.binary, "serve", str(place)], cwd=str(place), env=env or cx.env, capture_output=True, text=True, timeout=timeout)
        return p_.returncode, p_.stderr

    def held_lines(place):
        log = place / ".arbos" / "runtime" / "kernel.log"
        if not log.exists():
            return []
        out = []
        for l in log.read_text(errors="replace").splitlines():
            if "place_held" in l or "place_freed" in l:
                out.append(l)
        return out

    def classify(lines):
        return {
            "full": sum(1 for l in lines if "another kernel already serves" in l),
            "heartbeat": sum(1 for l in lines if "still held after" in l),
            "escalation": sum(1 for l in lines if "a person needs to look" in l),
            "freed": sum(1 for l in lines if "place_freed" in l),
        }

    @reg("lk-01-a-held-place-is-said-once-then-escalates-on-a-real-loop", tags=("lock", "slow"))
    def lk01(cx):
        """#441, on a real relaunch loop rather than a moved clock: a kernel holds the place; a supervisor relaunches
        every 2 s for 5.5 minutes (~160 starts). Expected in the place's kernel.log: one full `place_held` line naming
        the holder's pid, build and url; one heartbeat a minute; one error-level escalation after five minutes, not
        repeated within ten; every relaunch exits 3 with `place already served` on stderr."""
        place = cx.place
        place.mkdir(parents=True, exist_ok=True)
        holder = cx.kernel(tag="holder")
        cx.rec.expect(holder.start(), "holder-start", "the holding kernel did not come up")
        holder_pid = holder.proc.pid
        codes, phrases, t0 = [], 0, time.time()
        while time.time() - t0 < 330:
            code, err = relaunch(cx, place)
            codes.append(code)
            phrases += "place already served" in err
            time.sleep(2)
        lines = held_lines(place)
        c_ = classify(lines)
        cx.rec.notes.update({"relaunches": len(codes), "exit_codes": sorted(set(codes)), "stderr_phrase_every_time": phrases == len(codes), "kernel_log_place_held_lines": len(lines), "classes": c_, "first_line": next((l[:300] for l in lines if "another kernel already serves" in l), None), "escalation_line": next((l[:300] for l in lines if "a person needs to look" in l), None)})
        cx.rec.expect(len(codes) >= 100, "probe-too-few-relaunches", f"only {len(codes)} relaunches in 5.5 min; this run proves little")
        cx.rec.expect(set(codes) == {3}, "exit-code-not-3", f"relaunches exited {sorted(set(codes))}; a held place must exit 3 (EXIT_PLACE_HELD) every time")
        cx.rec.expect(phrases == len(codes), "phrase-missing-on-stderr", f"`place already served` was on stderr {phrases} of {len(codes)} times; the desktop parses that phrase", "arbos-kernel serve.rs say_held — the phrase is an interface")
        cx.rec.expect(c_["full"] == 1, "full-line-not-once", f"the full line appeared {c_['full']} times over {len(codes)} relaunches (kernel.log place_held lines: {len(lines)})", "arbos-kernel serve.rs HeldRecord (#441)")
        cx.rec.expect(4 <= c_["heartbeat"] <= 7, "heartbeat-cadence", f"{c_['heartbeat']} heartbeats over 5.5 min; expected about one a minute")
        cx.rec.expect(c_["escalation"] == 1, "escalation-did-not-escalate", f"{c_['escalation']} escalation line(s) after 5.5 min held; expected exactly one after five minutes")
        first = next((l for l in lines if "another kernel already serves" in l), "")
        cx.rec.expect(str(holder_pid) in first and "url" in first and "build" in first, "first-line-lacks-the-facts", f"the first line does not name pid {holder_pid}, build and url: {first[:300]}")
        holder.stop()
        cx.check()

    @reg("lk-02-held-record-in-a-read-only-runtime-folder", tags=("lock", "destructive-order"))
    def lk02(cx):
        """The record that makes 'once' possible (runtime/place-held.json, or the machine's temp folder since #441 at
        46477c88). Three shapes, six relaunches each, folders made read-only before any record exists: runtime/ read-only
        with temp writable (one long line expected); a stale runtime/ record that cannot be updated beside a writable temp
        (which copy is read?); both read-only (six short lines that say the record could not be kept, never the long line,
        never the escalation). qal-j19."""
        place = cx.place
        place.mkdir(parents=True, exist_ok=True)
        holder = cx.kernel(tag="holder")
        cx.rec.expect(holder.start(), "holder-start", "the holding kernel did not come up")
        runtime = place / ".arbos" / "runtime"
        record = runtime / "place-held.json"
        holder_pid = holder.proc.pid
        # The record may also live in the machine's temp folder (#441 at 46477c88). Two environments for the
        # relaunches: temp writable, and temp read-only (TMPDIR pointed at a folder with no write bit).
        tmp_ro = cx.scratch / "tmp-ro"
        tmp_ro.mkdir(exist_ok=True)
        tmp_ro.chmod(0o555)
        env_tmp_ro = {**cx.env, "TMPDIR": str(tmp_ro)}
        tmp_rw = cx.scratch / "tmp-rw"
        tmp_rw.mkdir(exist_ok=True)
        env_tmp_rw = {**cx.env, "TMPDIR": str(tmp_rw)}

        def burst(env, n=6):
            errs, codes = [], []
            for _ in range(n):
                code, err = relaunch(cx, place, env=env)
                codes.append(code)
                errs.append(err)
                time.sleep(0.2)
            return codes, errs

        def count(errs):
            return {"full": sum("another kernel already serves" in e for e in errs), "escalation": sum("a person needs to look" in e for e in errs), "short": sum("said in full" in e or "could not" in e.lower() or "record" in e.lower() for e in errs), "says_record_unkept": sum("could not" in e.lower() and "record" in e.lower() or "place-held" in e for e in errs)}

        results = {}
        codes_all = []
        # (a) no record, runtime/ read-only, temp writable: the record goes to temp; one long line in six.
        record.unlink(missing_ok=True)
        for f_ in tmp_rw.glob("arbos-place-held-*"):
            f_.unlink()
        runtime.chmod(0o555)
        try:
            c_, e_ = burst(env_tmp_rw)
        finally:
            runtime.chmod(0o755)
        codes_all += c_
        results["a_runtime_ro_temp_rw"] = {**count(e_), "sample": e_[-1][:220]}
        # (b) a stale, unwritable runtime record (six minutes old, escalated: false) beside a writable temp: which one
        # does the next start read? If runtime first, the temp copy never speaks and the escalation repeats.
        for f_ in tmp_rw.glob("arbos-place-held-*"):
            f_.unlink()
        now = now_ms()
        record.write_text(json.dumps({"holder_pid": holder_pid, "first_ms": now - 360_000, "last_said_ms": now - 360_000, "refusals": 180, "escalated": False}))
        runtime.chmod(0o555)
        try:
            c_, e_ = burst(env_tmp_rw)
        finally:
            runtime.chmod(0o755)
        codes_all += c_
        results["b_stale_runtime_record_ro_temp_rw"] = {**count(e_), "sample": e_[-1][:220]}
        # (c) nowhere to keep it: runtime/ and temp both read-only, no record anywhere → six short lines, no long, no escalation.
        record.unlink(missing_ok=True)
        runtime.chmod(0o555)
        try:
            c_, e_ = burst(env_tmp_ro)
        finally:
            runtime.chmod(0o755)
        codes_all += c_
        results["c_runtime_ro_temp_ro"] = {**count(e_), "sample": e_[-1][:220]}
        tmp_ro.chmod(0o755)
        cx.rec.notes.update({"exit_codes": sorted(set(codes_all)), "results": results})
        a, b, c3 = results["a_runtime_ro_temp_rw"], results["b_stale_runtime_record_ro_temp_rw"], results["c_runtime_ro_temp_ro"]
        cx.rec.expect(set(codes_all) <= {3}, "exit-code-not-3", f"relaunches exited {sorted(set(codes_all))} with folders read-only")
        cx.rec.expect(a["full"] == 1 and a["escalation"] == 0, "runtime-ro-temp-rw-not-once", f"runtime/ read-only with a writable temp: full line {a['full']} of 6, escalation {a['escalation']} — the temp fallback did not give 'once': {a['sample']}", "arbos-kernel serve.rs HeldRecord::save/paths (#441)")
        cx.rec.expect(b["escalation"] <= 1, "stale-runtime-record-shadows-the-temp-copy", f"a stale runtime/ record that cannot be updated, beside a writable temp: the error-level escalation went out {b['escalation']} of 6 — `load` reads runtime/ first, so the copy that is being kept is never the one read: {b['sample']}", "arbos-kernel serve.rs HeldRecord::load — prefer the newest record, or the one that save() last wrote")
        cx.rec.expect(c3["full"] == 0 and c3["escalation"] == 0 and c3["says_record_unkept"] >= 5, "nowhere-to-keep-it-not-short", f"runtime/ and temp both read-only: full {c3['full']}, escalation {c3['escalation']}, lines saying the record could not be kept {c3['says_record_unkept']} of 6 — expected six short lines that say so: {c3['sample']}", "arbos-kernel serve.rs say_held — a record kept nowhere means the short form, every time, saying why")
        holder.stop()
        cx.check()

    @reg("lk-03-holder-gone-clears-the-record-and-a-new-holder-starts-fresh", tags=("lock",))
    def lk03(cx):
        """When the holder goes, the first start that serves must clear runtime/place-held.json rather than inherit a
        permanently-refusing state; and a new holder afterwards gets its own first full line (keyed on its pid), not
        the old holder's heartbeat cadence."""
        place = cx.place
        place.mkdir(parents=True, exist_ok=True)
        record = place / ".arbos" / "runtime" / "place-held.json"
        holder = cx.kernel(tag="holder")
        cx.rec.expect(holder.start(), "holder-start", "the holding kernel did not come up")
        for _ in range(3):
            relaunch(cx, place)
            time.sleep(0.3)
        rec_before = json.loads(record.read_text()) if record.exists() else None
        holder.kill()
        time.sleep(1.0)
        # The supervisor's next start serves. Run it as the harness's own kernel so it can be stopped.
        served = cx.kernel(tag="served")
        ok = served.start()
        time.sleep(1.0)
        cleared = not record.exists()
        lines_after_serve = held_lines(place)
        served.stop()
        time.sleep(0.5)
        # A new holder, then a refusal: a fresh record for the new pid, first line in full again.
        holder2 = cx.kernel(tag="holder2")
        cx.rec.expect(holder2.start(), "holder2-start", "the second holding kernel did not come up")
        relaunch(cx, place)
        rec_after = json.loads(record.read_text()) if record.exists() else None
        c_ = classify(held_lines(place))
        cx.rec.notes.update({"record_before_holder_died": rec_before, "served_after_holder_died": ok, "record_cleared_by_serving_start": cleared, "record_for_new_holder": rec_after, "classes": c_, "freed_lines": [l[:200] for l in lines_after_serve if "free" in l.lower()]})
        cx.rec.expect(ok, "did-not-serve-after-holder-died", "the start after the holder died did not serve the place")
        cx.rec.expect(cleared, "stale-record-kept", "runtime/place-held.json survived a start that served: the next holder's refusals inherit its clock and count", "arbos-kernel serve.rs — clear the record on the first successful start (#441)")
        cx.rec.expect(rec_after is not None and rec_before is not None and rec_after.get("holder_pid") == holder2.proc.pid and rec_after.get("refusals") == 1, "new-holder-inherits-old-record", f"after a new holder the record is {rec_after}; expected holder_pid {holder2.proc.pid}, refusals 1")
        cx.rec.expect(c_["full"] == 2, "second-holder-not-said-in-full", f"the full line appeared {c_['full']} time(s); expected twice, once per holder")
        cx.rec.expect(c_["freed"] >= 1, "freed-not-said", "no `place_freed` line when the start after the holder died served the place")
        holder2.stop()
        cx.check()

    @reg("lk-04-removing-both-lock-files-does-not-let-a-second-kernel-in", tags=("lock", "after-failure"))
    def lk04(cx):
        """`qal-j40`: one place, one kernel — including after someone deletes the lock files.

        The place lock is `flock` on two files, `.arbos/lock` and `.arbos/runtime/lock`
        (`place.rs:132-148`). A lock lives on the open descriptor's **inode**, not on the path, so
        deleting both files leaves the holder's locks on unlinked inodes, held and unreachable. The
        arriving kernel creates fresh files at the same paths, locks those — different inodes — and
        succeeds. Two kernels then serve one place and neither says anything.

        `#450` stops the ordinary shapes: removing only `runtime/` is refused, because the lock
        takes both files and the other one still holds. It takes removing **both**, which is what
        someone resetting a place they believe is stuck would reach for, having read that the lock
        lives in two places.

        Staged from `deploy/af05c-runtime-and-lock-removed-probe.sh`, which reproduced this on every
        attempt against `arbos-kernel 0.2.0 cecd48e1bd76`. The probe is a script anyone can run; this
        is the check that runs every cycle, so a fix is noticed rather than waited for."""
        place = cx.place
        place.mkdir(parents=True, exist_ok=True)
        a = cx.kernel(tag="holder-a")
        cx.rec.expect(a.start(), "lk-04-holder-did-not-serve", "the first kernel never came up, so nothing was held to take")
        a_pid = a.proc.pid if a.proc else None

        shutil.rmtree(place / ".arbos" / "runtime", ignore_errors=True)
        (place / ".arbos" / "lock").unlink(missing_ok=True)
        time.sleep(2)
        cx.rec.notes["holder_alive_after_removal"] = a.alive()

        b = cx.kernel(tag="arriving-b")
        b_served = b.start()
        time.sleep(1)
        both_live = bool(a.alive() and b.alive())
        said = b.stderr_text()
        refused = any(phrase in said.lower() for phrase in ("already served", "place is held", "held by"))
        cx.rec.notes.update({
            "holder_pid": a_pid,
            "arriving_pid": b.proc.pid if b.proc else None,
            "arriving_wrote_its_own_kernel_json": b_served,
            "both_alive": both_live,
            "arriving_refused_out_loud": refused,
            "arriving_said": said[-300:],
        })

        # The contract, stated so a pass means something: while the holder is alive, a second kernel
        # must not end up serving the same place. Refusing out loud is the good outcome; failing to
        # start for any other reason is acceptable here too — what must not happen is two servers.
        cx.rec.expect(
            not (b_served and both_live),
            "lk-04-two-kernels-serve-one-place",
            f"the holder (pid {a_pid}) is still alive and the arriving kernel (pid "
            f"{b.proc.pid if b.proc else '?'}) took the place anyway after both lock files were "
            f"removed; it {'said nothing about the place being held' if not refused else 'did warn, yet served'}"
            f" (qal-j40)",
        )
        b.stop()
        a.stop()
        cx.check()

    # ── first-match readers: a stale copy in the first place, a live one in the second ──
    @reg("fm-01-stale-checkpoint-sidecar-from-a-cut-turn-is-taken-for-the-new-turn-at-the-same-line", tags=("first-match", "rewind", "destructive-order"))
    def fm01(cx):
        """settle_tree reads checkpoints.d/<line>.json when a checkpoint's tree is still pending, and accepts it if its
        HEAD matches. Line numbers are transcript lines: after a rewind, new turns reuse the cut turns' lines, and the
        cut turns' sidecars are not removed. Stage it: five turns, rewind to 3 (f3..f5 gone), new turns 3' and 4' with
        the tree delayed so 4' is still pending, rewind to 4'. The right tree is {f1, f2, g3}; the stale sidecar says
        {f1, f2, f3} — and HEAD never moved, so the guard passes it."""
        place = cx.place
        place.mkdir(parents=True, exist_ok=True)
        g = lambda *a: subprocess.run(["git", "-c", "user.name=qa", "-c", "user.email=qa@qa", *a], cwd=place, capture_output=True, text=True)
        for args in (["init", "-q"], ["config", "user.name", "qa"], ["config", "user.email", "qa@qa"], ["commit", "-q", "--allow-empty", "-m", "start"]):
            g(*args)
        (place / ".gitignore").write_text(".arbos/\n")
        g("add", ".gitignore")
        g("commit", "-q", "-m", "ignore .arbos")
        cps = place / ".arbos" / "agents" / "root" / "checkpoints.jsonl"
        sidecars = place / ".arbos" / "agents" / "root" / "checkpoints.d"

        def records():
            return [json.loads(l) for l in cps.read_text().splitlines() if l.strip()] if cps.exists() else []

        def settled(n, secs=20):
            end = time.time() + secs
            while time.time() < end:
                r = records()
                if len(r) >= n and all(x.get("work") or x.get("clean") for x in r[:n]):
                    return r
                time.sleep(0.2)
            return records()

        replies_a = []
        for i, w in enumerate(("first", "second", "third", "fourth", "fifth"), 1):
            replies_a.append({"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": f"echo {w} > f{i}.txt", "description": f"write f{i}"}}]})
            replies_a.append({"agent": "root", "content": w})
        k = cx.kernel(tag="kernel-a", extra_args=["--provider", "replay", "--replies", str(replies_file(cx, replies_a))])
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        c = k.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        for t in ("one", "two", "three", "four", "five"):
            c.user("root", t)
            cx.rec.expect(c.wait_turn("root", "idle", 60) is not None, "turn-never-ended", f"turn {t!r} never ended")
        recs = settled(5)
        old_lines = [r.get("line") for r in recs]
        c.send({"type": "rewind", "agent": "root", "turn": 3, "files": True})
        c.wait(lambda f: f.get("type") == "rewound" and f.get("agent") == "root", 15, "the rewound frame")
        follow = c.wait(lambda f: (f.get("type") == "rewound" and f.get("restored") is not None) or f.get("type") == "error", 45, "the restore's report")
        time.sleep(0.5)
        files_after_first = sorted(p_.name for p_ in place.glob("*.txt"))
        stale = sorted(p_.name for p_ in sidecars.glob("*.json")) if sidecars.exists() else []
        k.stop()
        cx.rec.notes.update({"old_checkpoint_lines": old_lines, "first_rewind": follow, "files_after_first_rewind": files_after_first, "sidecars_left_after_rewind": stale})
        cx.rec.expect(files_after_first == ["f1.txt", "f2.txt"], "first-rewind-wrong", f"after rewinding to turn 3 the files are {files_after_first}; the probe needs f1, f2")
        # Kernel B: the tree delayed, so a turn that does not write ends with its record still pending.
        replies_b = [
            {"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": "echo g3 > g3.txt", "description": "write g3"}}]},
            {"agent": "root", "content": "third again"},
            {"agent": "root", "content": "noted, nothing to write"},
        ]
        k2 = cx.kernel(tag="kernel-b", extra_args=["--provider", "replay", "--replies", str(replies_file(cx, replies_b))])
        k2.env["ARBOS_TEST_TREE_DELAY_MS"] = "8000"
        cx.rec.expect(k2.start(), "kernel-b-start", "the second kernel did not come up")
        c2 = k2.attach()
        c2.wait(lambda f: f.get("type") == "snapshot", 5)
        c2.user("root", "three again")
        cx.rec.expect(c2.wait_turn("root", "idle", 60) is not None, "turn-never-ended", "turn 3' never ended")
        c2.user("root", "four again")
        cx.rec.expect(c2.wait_turn("root", "idle", 60) is not None, "turn-never-ended", "turn 4' never ended")
        recs2 = records()
        new4 = recs2[3] if len(recs2) >= 4 else {}
        stale_for_new4 = (sidecars / f"{new4.get('line')}.json") if new4 else None
        stale_json = json.loads(stale_for_new4.read_text()) if stale_for_new4 and stale_for_new4.exists() else None
        before = tree_state(place)
        cx.rec.notes.update({"new_turn4_record": {k_: str(v)[:40] for k_, v in new4.items()}, "stale_sidecar_for_that_line": stale_json and {k_: str(v)[:40] for k_, v in stale_json.items()}, "files_before_second_rewind": sorted(before["files"])})
        cx.rec.expect(new4.get("work_error") and "pending" in str(new4.get("work_error")).lower() or "being saved" in str(new4.get("work_error", "")).lower(), "probe-record-not-pending", f"turn 4''s record is not pending ({new4}); the sidecar is never consulted, this run proves nothing")
        cx.rec.expect(stale_json is not None and stale_json.get("work") and new4.get("line") in old_lines, "probe-no-stale-sidecar", f"no cut turn's sidecar at line {new4.get('line')} (old lines {old_lines}, sidecars {stale}); the collision did not happen, this run proves nothing")
        c2.send({"type": "rewind", "agent": "root", "turn": 4, "files": True})
        c2.wait(lambda f: f.get("type") == "rewound" and f.get("agent") == "root", 15, "the rewound frame")
        follow2 = c2.wait(lambda f: (f.get("type") == "rewound" and f.get("restored") is not None) or f.get("type") == "error", 45, "the restore's report")
        time.sleep(0.5)
        files_after = sorted(p_.name for p_ in place.glob("*.txt"))
        cx.rec.notes.update({"second_rewind": follow2, "files_after_second_rewind": files_after})
        cx.rec.expect("f3.txt" not in files_after, "stale-sidecar-restored-a-cut-turns-tree", f"rewinding to the new turn 4 brought back f3.txt, a file the earlier rewind removed: the restore took the cut turn's sidecar at the same line (same HEAD) for the new turn's tree — files now {files_after}, expected f1, f2, g3", "arbos-engine tools::git settle_tree — a sidecar is matched by line and HEAD; a cut turn's sidecar at the same line passes both. Remove cut turns' sidecars on rewind, or key the sidecar on the record's ts")
        cx.rec.expect("g3.txt" in files_after, "new-turns-file-lost", f"g3.txt, written by the new turn 3', is gone after rewinding to the new turn 4: {files_after}")
        k2.stop()
        # The stale sidecar past the end of the transcript is what this scenario stages, so the
        # standing rule naming it is this working, not a second finding (it is kept in the notes).
        cx.check(staged=("checkpoint-past-the-transcript",))

    @reg("rw-09-clean-that-fails-is-in-what-restored-says", tags=("rewind", "misreport"))
    def rw09(cx):
        """#419's second claim: a later turn left an untracked folder git cannot remove (a directory with no write
        bit, a file inside). Rewind with files: true. The tree restore itself works; `git clean` fails on that folder.
        The person must be told the leftover exists — "restored" alone is a false claim."""
        place = cx.place
        place.mkdir(parents=True, exist_ok=True)
        g = lambda *a: subprocess.run(["git", "-c", "user.name=qa", "-c", "user.email=qa@qa", *a], cwd=place, capture_output=True, text=True)
        for args in (["init", "-q"], ["config", "user.name", "qa"], ["config", "user.email", "qa@qa"], ["commit", "-q", "--allow-empty", "-m", "start"]):
            g(*args)
        (place / ".gitignore").write_text(".arbos/\n")
        g("add", ".gitignore")
        g("commit", "-q", "-m", "ignore .arbos")
        replies = []
        for i, word in enumerate(("first", "second", "third"), 1):
            replies.append({"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": f"echo {word} > f{i}.txt", "description": f"write f{i}"}}]})
            replies.append({"agent": "root", "content": word})
        k = cx.kernel(extra_args=["--provider", "replay", "--replies", str(replies_file(cx, replies))])
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        c = k.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        for t in ("one", "two", "three"):
            c.user("root", t)
            cx.rec.expect(c.wait_turn("root", "idle", 60) is not None, "turn-never-ended", f"turn {t!r} never ended")
        stuck = place / "later-dir"
        stuck.mkdir()
        (stuck / "keep.txt").write_text("cannot be removed\n")
        stuck.chmod(0o555)
        try:
            c.send({"type": "rewind", "agent": "root", "turn": 3, "files": True})
            c.wait(lambda f: f.get("type") == "rewound" and f.get("agent") == "root", 15, "the rewound frame")
            follow = c.wait(lambda f: (f.get("type") == "rewound" and f.get("restored") is not None) or f.get("type") == "error", 30, "the restore's report")
            time.sleep(0.5)
            still_there = (stuck / "keep.txt").exists()
            files = sorted(p_.name for p_ in place.glob("f*.txt"))
            cx.rec.notes["restore_report"] = follow
            cx.rec.notes["files_after"] = files
            cx.rec.notes["leftover_still_there"] = still_there
            said = json.dumps(follow or {})
            cx.rec.expect(files == ["f1.txt", "f2.txt"], "files-not-restored", f"after rewinding to turn 3 the files are {files}, expected f1, f2")
            cx.rec.expect(still_there, "probe-did-not-hold", "the unremovable folder was removed after all; this probe did not exercise a failing clean")
            if still_there and follow:
                cx.rec.expect("remain" in said or "clean" in said.lower() or follow.get("type") == "error", "failed-clean-called-restored", f"git clean could not remove later-dir/ yet the client was told: {said[:300]}", "arbos-engine tools::git restore — clean's status was let _ (#419)")
        finally:
            stuck.chmod(0o755)
        evs, bad = transcript(cx.place, "root")
        cx.rec.expect(not bad, "transcript-corrupt", f"bad lines: {bad}")
        k.stop()
        cx.check()

    rw_scenario("rw-04-rewind-with-files-in-a-repo-without-git-identity", "The same rewind in a repository with no git user.name/user.email (a new user's fresh place): the checkpoint's work-tree commit fails silently, so `files: true` resets to HEAD and cleans — deleting the kept turns' uncommitted files too — and reports success (qal-j08).", "no-identity")


    # ── the qal-j08 family: a failed read becomes "nothing", then a write trusts it ──
    @reg("sw-01-unreadable-notes-page-is-rewritten-empty", tags=("silent-write", "destructive"))
    def sw01(cx):
        """`notes::load` reads the project page with `unwrap_or_default()`: any read failure (EACCES here; EIO, a lock, a partial view on a network mount in life) parses as an empty page, and the next `plan` call writes that empty page over the real one by tmp+rename — the goal, the checklist, the prose gone, the tool reporting success. Expected: a page that could not be read is not rewritten; the call fails with the reason."""
        arbos = cx.place / ".arbos"
        arbos.mkdir(exist_ok=True)
        page = arbos / "notes.md"
        original = "# Shapes\n\n## Goal\nKeep the geometry helpers correct and documented.\n\n## Now\n- [ ] [Kickoff](docs/project-context.md) — ready\n- [ ] Fix area() — in progress\n- [x] Seed the repo — done 09-16\n\n## Notes\nJacob wants British spelling in the CHANGELOG.\nThe worker reports to root, root reports once.\n"
        page.write_text(original)
        lines = [
            {"agent": "root", "content": "", "calls": [{"name": "plan", "arguments": {"items": ["- [ ] Add perimeter() tests — ready"]}}]},
            {"agent": "root", "content": "Plan updated."},
        ]
        k = cx.kernel(extra_args=["--provider", "replay", "--replies", str(replies_file(cx, lines))])
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        c = k.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        # The page becomes unreadable (the injector); it still holds every byte.
        os.chmod(page, 0)
        try:
            c.user("root", "Add a plan item: perimeter tests.")
            cx.rec.expect(c.wait_turn("root", "idle", 60) is not None, "sw-01-turn-never-ended", "the turn never ended")
        finally:
            try:
                os.chmod(page, 0o644)
            except Exception:  # noqa: BLE001
                pass
        time.sleep(0.5)
        after = page.read_text(errors="replace") if page.exists() else None
        evs, _ = transcript(cx.place, "root")
        plan_calls = [e for e in evs if e.get("kind") == "tool" and e.get("name") == "plan"]
        plan_said = [(e.get("error") or str(e.get("body") or e.get("result") or ""))[:200] for e in plan_calls]
        kept = after is not None and all(x in after for x in ("Keep the geometry helpers", "Fix area()", "British spelling", "Seed the repo"))
        cx.rec.notes.update({"page_after": (after or "")[:400], "page_kept": kept, "plan_said": plan_said, "page_bytes_before_after": [len(original), len(after or "")]})
        cx.rec.expect(kept, "sw-01-page-rewritten-from-nothing", f"the project page was replaced after one unreadable read: {len(original)} → {len(after or '')} bytes; goal/checklist/notes gone; the plan tool said {plan_said[:1]}", "arbos-core notes.rs load() unwrap_or_default + save_path tmp+rename")
        cx.rec.expect(not plan_calls or any(e.get("error") for e in plan_calls) or kept, "sw-01-success-reported-over-a-lost-page", "the plan tool reported success while the page it could not read was being replaced")


    @reg("sw-02-stale-undo-mark-resets-past-committed-work", tags=("silent-write", "destructive"))
    def sw02(cx):
        """Turn 1 commits A. Before turn 2 the mark file is made unwritable (the injector for a full disk or an unwritable runtime/), so turn 2's start cannot record HEAD=A and the mark still says HEAD0. Turn 2 commits B, then calls `undo` — meant to drop only turn 2's work. `undo` reads the stale mark, `git reset --hard HEAD0` + `git clean -fd`: commit A is gone from the branch, its files gone from the tree, and the tool says "restored HEAD0"."""
        place = cx.place
        for args in (["init", "-q"], ["config", "user.name", "qa"], ["config", "user.email", "qa@qa"], ["commit", "-q", "--allow-empty", "-m", "start"]):
            subprocess.run(["git", *args], cwd=place, capture_output=True)
        (place / ".gitignore").write_text(".arbos/\n")
        subprocess.run(["git", "add", ".gitignore"], cwd=place, capture_output=True)
        subprocess.run(["git", "commit", "-q", "-m", "ignore"], cwd=place, capture_output=True)
        subprocess.run(["git", "checkout", "-q", "-b", "work"], cwd=place, capture_output=True)  # not a protected branch
        head0 = subprocess.run(["git", "rev-parse", "HEAD"], cwd=place, capture_output=True, text=True).stdout.strip()
        # root as a plain agent with `undo` on its allowlist (a coordinator has neither undo nor project writes).
        root = place / ".arbos" / "agents" / "root"
        (root / "pages").mkdir(parents=True, exist_ok=True)
        (root / "jobs").mkdir(exist_ok=True)
        (root / "agent.md").write_text(f"name: root\ntitle: \nparent: \npaused: false\nmodel: inherit\nallowlist: ls, read, write, edit, bash, undo, changes, plan, say\nreadonly: false\ncwd: {place}\n")
        (root / "transcript.jsonl").touch()
        (place / ".arbos" / "project.toml").write_text('schema = 2\n\n[git]\nprotected = []\n')
        lines = [
            {"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": "echo A > a.txt && git add a.txt && git commit -q -m 'turn one: A' && git rev-parse HEAD", "description": "commit A"}}]},
            {"agent": "root", "content": "Committed A."},
            {"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": "echo B > b.txt && git add b.txt && git commit -q -m 'turn two: B' && git rev-parse HEAD", "description": "commit B"}}]},
            {"agent": "root", "content": "", "calls": [{"name": "undo", "arguments": {}}]},
            {"agent": "root", "content": "Undone."},
        ]
        k = cx.kernel(extra_args=["--provider", "replay", "--replies", str(replies_file(cx, lines))])
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        c = k.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        c.user("root", "Commit A.")
        cx.rec.expect(c.wait_turn("root", "idle", 60) is not None, "sw-02-turn-one-never-ended", "turn one never ended")
        head_a = subprocess.run(["git", "rev-parse", "HEAD"], cwd=place, capture_output=True, text=True).stdout.strip()
        mark = place / ".arbos" / "runtime" / "checkpoint"
        mark_before = mark.read_text().strip() if mark.exists() else None
        # The injector: the mark cannot be rewritten at turn 2's start (a full disk, an unwritable runtime/ — silent either way).
        os.chmod(mark, 0o444)
        os.chmod(mark.parent, 0o555)
        try:
            c.user("root", "Commit B, then undo this turn.")
            cx.rec.expect(c.wait_turn("root", "idle", 60) is not None, "sw-02-turn-two-never-ended", "turn two never ended")
        finally:
            os.chmod(mark.parent, 0o755)
            os.chmod(mark, 0o644)
        head_after = subprocess.run(["git", "rev-parse", "HEAD"], cwd=place, capture_output=True, text=True).stdout.strip()
        log = subprocess.run(["git", "log", "--oneline"], cwd=place, capture_output=True, text=True).stdout.strip().splitlines()
        evs, _ = transcript(place, "root")
        undo_said = [(e.get("error") or str(e.get("body") or e.get("result") or ""))[:120] for e in evs if e.get("kind") == "tool" and e.get("name") == "undo"]
        cx.rec.notes.update({"head0": head0[:12], "head_after_turn_one": head_a[:12], "mark_before_turn_two": (mark_before or "")[:12], "mark_after": mark.read_text().strip()[:12] if mark.exists() else None, "head_after_undo": head_after[:12], "log_after": log[:4], "a_txt_exists": (place / "a.txt").exists(), "undo_said": undo_said})
        cx.rec.expect(head_a != head0, "sw-02-turn-one-did-not-commit", "turn one's commit did not happen; nothing to measure")
        if head_a != head0:
            # Two honest outcomes: `undo` restored exactly to turn two's start (A), or it refused and reset nothing
            # (HEAD still at B, a refusal that names the reason). The dishonest one: HEAD behind A — commit A destroyed.
            a_intact = (place / "a.txt").exists() and subprocess.run(["git", "merge-base", "--is-ancestor", head_a, "HEAD"], cwd=place, capture_output=True).returncode == 0
            refused = any(("nothing reset" in u.lower() or "no checkpoint" in u.lower()) for u in undo_said)
            cx.rec.notes["outcome"] = "restored-to-turn-start" if head_after == head_a else ("refused-nothing-reset" if refused and a_intact else "destroyed")
            cx.rec.expect(a_intact, "sw-02-undo-destroyed-committed-work", f"`undo` in turn two reset to {head_after[:12]} (turn two started at {head_a[:12]}; the mark said {(mark_before or '?')[:12]}): commit A and a.txt are gone, the tool said {undo_said[:1]}", "arbos-engine tools/git.rs snapshot() ignored write + undo() trusting the mark")
            cx.rec.expect(head_after == head_a or refused, "sw-02-undo-silent", f"`undo` neither restored to turn two's start nor said why it refused: HEAD {head_after[:12]}, said {undo_said[:1]}")


    # ── #390's fourth hole, and its refusal path ────────────────────────────
    def user_repo(place, identity=True):
        for args in (["init", "-q"], ["commit", "-q", "--allow-empty", "-m", "start"]):
            subprocess.run(["git", "-c", "user.name=qa", "-c", "user.email=qa@qa", *args], cwd=place, capture_output=True)
        if identity:
            subprocess.run(["git", "config", "user.name", "qa"], cwd=place, capture_output=True)
            subprocess.run(["git", "config", "user.email", "qa@qa"], cwd=place, capture_output=True)
        (place / ".gitignore").write_text(".arbos/\n")
        subprocess.run(["git", "add", ".gitignore"], cwd=place, capture_output=True)
        subprocess.run(["git", "-c", "user.name=qa", "-c", "user.email=qa@qa", "commit", "-q", "-m", "ignore"], cwd=place, capture_output=True)
        subprocess.run(["git", "checkout", "-q", "-b", "work"], cwd=place, capture_output=True)
        root = place / ".arbos" / "agents" / "root"
        (root / "pages").mkdir(parents=True, exist_ok=True)
        (root / "jobs").mkdir(exist_ok=True)
        (root / "agent.md").write_text(f"name: root\ntitle: \nparent: \npaused: false\nmodel: inherit\nallowlist: ls, read, write, edit, bash, undo, changes, plan, say\nreadonly: false\ncwd: {place}\n")
        (root / "transcript.jsonl").touch()
        (place / ".arbos" / "project.toml").write_text('schema = 2\n\n[git]\nprotected = []\n')

    @reg("sw-03-undo-keeps-the-users-own-untracked-files", tags=("silent-write", "destructive", "undo"))
    def sw03(cx):
        """A project with the user's own untracked files — notes, a scratch folder, a photo — before Arbos ever ran. The agent does one turn of work and calls `undo`. The user's files must still be there afterwards; on the old kernel `undo` ran `git clean -fd` and deleted every untracked file in the project."""
        place = cx.place
        user_repo(place)
        (place / "my-notes.txt").write_text("things I typed before Arbos existed\n")
        (place / "scratch").mkdir()
        (place / "scratch" / "ideas.md").write_text("- an idea\n")
        (place / "holiday.jpg").write_bytes(b"\xff\xd8\xff\xe0 not really a jpeg\n")
        mine = ["my-notes.txt", "scratch/ideas.md", "holiday.jpg"]
        lines = [
            {"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": "echo draft > draft.txt", "description": "some work"}}]},
            {"agent": "root", "content": "", "calls": [{"name": "undo", "arguments": {}}]},
            {"agent": "root", "content": "Undone."},
        ]
        k = cx.kernel(extra_args=["--provider", "replay", "--replies", str(replies_file(cx, lines))])
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        c = k.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        c.user("root", "Write a draft, then undo it.")
        cx.rec.expect(c.wait_turn("root", "idle", 60) is not None, "sw-03-turn-never-ended", "the turn never ended")
        evs, _ = transcript(place, "root")
        undo_said = [(e.get("error") or str(e.get("body") or e.get("result") or ""))[:160] for e in evs if e.get("kind") == "tool" and e.get("name") == "undo"]
        survived = [m for m in mine if (place / m).exists()]
        lost = [m for m in mine if not (place / m).exists()]
        cx.rec.notes.update({"users_files_survived": survived, "users_files_lost": lost, "draft_after": (place / "draft.txt").exists(), "undo_said": undo_said})
        cx.rec.expect(not lost, "sw-03-undo-deleted-the-users-files", f"`undo` deleted the user's own pre-existing untracked files: {lost}; the tool said {undo_said[:1]}", "arbos-engine tools/git.rs undo → git clean -fd (#390)")
        cx.rec.expect(bool(undo_said), "sw-03-undo-not-run", "the undo tool did not run")

    @reg("rw-05-old-kernels-checkpoint-refused-cleanly", tags=("rewind", "history", "compat"))
    def rw05(cx):
        """Checkpoints written by an OLDER kernel (ARBOS_QA_OLD_KERNEL) on a real place; then the current kernel takes over the place and the user rewinds with files: true. Expected on #390: the transcript is rewound, nothing is deleted, and the refusal is said in words a person can read — the case Jacob meets on every place he already has. Fails on the old kernel itself (it trusts the record) or if the refusal reads as a failure with no way on."""
        old = os.environ.get("ARBOS_QA_OLD_KERNEL")
        if not old or not Path(old).exists():
            cx.rec.notes["skipped"] = "ARBOS_QA_OLD_KERNEL not set: no older kernel to write the old-format checkpoints"
            return
        place = cx.place
        user_repo(place)
        lines = [
            {"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": "echo one > f1.txt", "description": "f1"}}]},
            {"agent": "root", "content": "first"},
            # Turn 2 commits, so at turn 3's start the tree equals HEAD: an older kernel records that checkpoint as
            # `work: None` — the same record it wrote when the checkpoint FAILED — which is why the new kernel cannot
            # trust it. This is the common shape on a real place: a rewind to the turn right after a commit.
            {"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": "echo two > f2.txt && git add -A && git commit -q -m two", "description": "f2, committed"}}]},
            {"agent": "root", "content": "second"},
            {"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": "echo three > f3.txt", "description": "f3"}}]},
            {"agent": "root", "content": "third"},
        ]
        rf = replies_file(cx, lines)
        kold = cx.kernel(tag="kernel-old", extra_args=["--provider", "replay", "--replies", str(rf)])
        kold.binary = old
        cx.rec.expect(kold.start(), "rw-05-old-kernel-start", "the older kernel did not come up")
        c = kold.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        for t in ("one", "two", "three"):
            c.user("root", t)
            cx.rec.expect(c.wait_turn("root", "idle", 60) is not None, "rw-05-old-turn-never-ended", f"turn {t!r} never ended on the older kernel")
        c.close()
        kold.stop()
        cps = (place / ".arbos" / "agents" / "root" / "checkpoints.jsonl").read_text().splitlines()
        cx.rec.notes["old_checkpoints"] = [l[:120] for l in cps]
        cx.rec.notes["old_kernel"] = subprocess.run([old, "--version"], capture_output=True, text=True).stdout.strip()
        files_before = sorted(p_.name for p_ in place.glob("f*.txt"))
        # The current kernel takes the place over; the user rewinds to turn 3 with files.
        knew = cx.kernel(tag="kernel-new", extra_args=["--provider", "replay", "--replies", str(rf)])
        cx.rec.expect(knew.start(), "rw-05-new-kernel-start", "the current kernel did not come up on the old place")
        c2 = knew.attach()
        c2.wait(lambda f: f.get("type") == "snapshot", 5)
        before_n = len(transcript(place, "root")[0])
        c2.send({"type": "rewind", "agent": "root", "turn": 3, "files": True})
        first = c2.wait(lambda f: f.get("type") == "rewound" and f.get("agent") == "root", 15, "rewound")
        follow = c2.wait(lambda f: (f.get("type") == "rewound" and f.get("restored") is not None) or f.get("type") == "error", 30, "the restore's report")
        time.sleep(1)
        evs, _ = transcript(place, "root")
        files_after = sorted(p_.name for p_ in place.glob("f*.txt"))
        notices = [e.get("text", "") for e in evs if e.get("kind") == "notice"]
        said = json.dumps(follow or {})
        cx.rec.notes.update({"files_before": files_before, "files_after": files_after, "transcript_lines": [before_n, len(evs)], "rewound_first": {k_: str(v)[:80] for k_, v in (first or {}).items()}, "follow": {k_: str(v)[:200] for k_, v in (follow or {}).items()}, "notices": [n[:200] for n in notices[-3:]]})
        cx.rec.expect(len(evs) < before_n and any(e.get("kind") == "user" and e.get("text") == "two" for e in evs) and not any(e.get("kind") == "user" and e.get("text") == "three" for e in evs), "rw-05-transcript-not-rewound", f"the transcript was not rewound to turn 3 ({before_n} → {len(evs)} lines)")
        cx.rec.expect(files_after == files_before, "rw-05-files-deleted-on-an-untrusted-record", f"files changed under a checkpoint the new kernel should not trust: {files_before} → {files_after}", "arbos-engine tools/git.rs restore knows_tree (#390)")
        refused = follow is not None and (follow.get("type") == "error" or "left as they are" in said or "no checkpoint of the working tree" in said)
        cx.rec.expect(refused, "rw-05-old-record-trusted", f"the new kernel did not refuse the old checkpoint: {said[:200]}")
        readable = "transcript is rewound" in said or any("transcript is rewound" in n or "files left as they are" in n for n in notices)
        cx.rec.expect(readable, "rw-05-refusal-unreadable", f"the refusal does not tell the user what happened and what did not: {said[:200]} / notices {notices[-1:]}")


    # ── after #392: the refusals must not have eaten the ordinary path ─────
    @reg("sw-04-healthy-undo-still-undoes", tags=("undo", "regression"))
    def sw04(cx):
        """Turn 1 commits A. Turn 2 edits a tracked file and writes an untracked draft, then calls `undo`. Expected: the tracked edit is gone, the draft is gone (the turn's own work), commit A and `a.txt` stay, HEAD is back at A, and the tool says "restored …" — not a refusal, not silence."""
        place = cx.place
        user_repo(place)
        lines = [
            {"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": "echo A > a.txt && git add a.txt && git commit -q -m 'A'", "description": "commit A"}}]},
            {"agent": "root", "content": "Committed A."},
            {"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": "echo changed >> a.txt && echo draft > draft.txt", "description": "turn two's work"}}]},
            {"agent": "root", "content": "", "calls": [{"name": "undo", "arguments": {}}]},
            {"agent": "root", "content": "Undone."},
        ]
        k = cx.kernel(extra_args=["--provider", "replay", "--replies", str(replies_file(cx, lines))])
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        c = k.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        c.user("root", "Commit A.")
        cx.rec.expect(c.wait_turn("root", "idle", 60) is not None, "sw-04-turn-one-never-ended", "turn one never ended")
        head_a = subprocess.run(["git", "rev-parse", "HEAD"], cwd=place, capture_output=True, text=True).stdout.strip()
        c.user("root", "Edit a.txt, write a draft, then undo this turn.")
        cx.rec.expect(c.wait_turn("root", "idle", 60) is not None, "sw-04-turn-two-never-ended", "turn two never ended")
        head_after = subprocess.run(["git", "rev-parse", "HEAD"], cwd=place, capture_output=True, text=True).stdout.strip()
        a_text = (place / "a.txt").read_text() if (place / "a.txt").exists() else None
        evs, _ = transcript(place, "root")
        undo_said = [(e.get("error") or str(e.get("body") or e.get("result") or ""))[:200] for e in evs if e.get("kind") == "tool" and e.get("name") == "undo"]
        undo_err = [e.get("error") for e in evs if e.get("kind") == "tool" and e.get("name") == "undo" and e.get("error")]
        cx.rec.notes.update({"head_a": head_a[:12], "head_after": head_after[:12], "a_txt": a_text, "draft_exists": (place / "draft.txt").exists(), "undo_said": undo_said, "undo_error": undo_err})
        cx.rec.expect(not undo_err and undo_said and "restored" in undo_said[-1].lower(), "sw-04-healthy-undo-refused", f"an ordinary undo did not restore: {undo_said[-1:] or undo_err}", "arbos-engine tools/git.rs undo (#390/#392 refusals)")
        cx.rec.expect(head_after == head_a and a_text == "A\n", "sw-04-tracked-edit-not-undone", f"after undo HEAD={head_after[:12]} (A={head_a[:12]}), a.txt={a_text!r}")
        cx.rec.expect(not (place / "draft.txt").exists(), "sw-04-turns-own-draft-kept", "the turn's own untracked draft survived its undo")

    # ── the census's misreport-only markers, second look ───────────────────
    @reg("sw-05-failed-migration-rename-doubles-crons-and-tasks", tags=("silent-write", "destructive", "migration"))
    def sw05(cx):
        """The one-time migration turns a legacy plan.jsonl into subscriptions, inbox files and notes lines, then renames plan.jsonl aside with `let _ = rename(...)`. If that rename fails (here: the agent folder is not writable for the rename while its subfolders are; in life a partial view or a permissions slip), the next kernel start finds plan.jsonl again and migrates again — the standing cron now exists twice and fires twice, the pending task is queued twice. A marker judged misreport-only reaches a doubled side effect."""
        arbos = cx.place / ".arbos"
        root = arbos / "agents" / "root"
        for d in ("pages", "jobs", "subscriptions", "inbox"):
            (root / d).mkdir(parents=True, exist_ok=True)
        (root / "agent.md").write_text(f"name: root\ntitle: \nparent: \npaused: false\nmodel: inherit\nallowlist: ls, read, bash, plan, say\nreadonly: false\ncwd: {cx.place}\n")
        (root / "transcript.jsonl").touch()
        now = now_ms()
        rows = [
            {"id": 1, "parent": 0, "seq": 0, "goal": "tick every 30s", "when": {"every_ms": 30_000, "next_due_ms": now + 3_600_000}, "do": {"kind": "shell", "cmd": "echo legacy-tick >> ticks.txt"}, "status": "pending", "origin": "user", "created_ms": now, "updated_ms": now},
            {"id": 2, "parent": 0, "seq": 1, "goal": "Reply with the single word MIGRATED.", "when": {"wake": False}, "do": {"kind": "agent"}, "status": "pending", "origin": "user", "created_ms": now, "updated_ms": now},
        ]
        (root / "plan.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
        # The injector: the agent folder itself is read-only (rename needs a writable parent), its subfolders are not.
        os.chmod(root, 0o555)
        try:
            for i in (1, 2):
                k = cx.kernel(tag=f"kernel-{i}", extra_args=["--provider", "replay", "--replies", str(replies_file(cx, [{"agent": "root", "content": "hi"}]))])
                cx.rec.expect(k.start(), f"sw-05-start-{i}", f"kernel start {i} did not come up")
                time.sleep(2.5)
                k.stop()
        finally:
            os.chmod(root, 0o755)
        subs = sorted(p_.name for p_ in (root / "subscriptions").glob("*.toml"))
        crons = [p_ for p_ in (root / "subscriptions").glob("*.toml") if "legacy-tick" in p_.read_text(errors="replace")]
        inbox = sorted(p_.name for p_ in (root / "inbox").glob("*")) if (root / "inbox").exists() else []
        plan_left = (root / "plan.jsonl").exists()
        evs, _ = transcript(cx.place, "root")
        notices = [e.get("text", "") for e in evs if e.get("kind") == "notice"]
        blocked_notice = next((n for n in notices if "migration" in n.lower()), None)
        cx.rec.notes.update({"plan_jsonl_left": plan_left, "subscriptions": subs, "cron_copies": len(crons), "inbox_files": inbox, "migration_notice": (blocked_notice or "")[:300]})
        if plan_left and not crons:
            # The blocked outcome (#392 @ 92f6eb59): the person must be able to read what happened, what to do, and still use the place.
            cx.rec.expect(blocked_notice is not None, "sw-05-blocked-in-silence", "the migration was blocked (nothing migrated) but the transcript carries no notice saying so", "arbos-kernel migrate.rs Claim::Blocked notice")
            if blocked_notice:
                actionable = "plan.jsonl" in blocked_notice and ("fix" in blocked_notice.lower() or "start" in blocked_notice.lower()) and ("permission" in blocked_notice.lower() or "denied" in blocked_notice.lower() or "could not" in blocked_notice.lower())
                cx.rec.expect(actionable, "sw-05-blocked-notice-not-actionable", f"the blocked notice does not name the file, the cause and the action: {blocked_notice[:200]!r}")
            # Usable while blocked: a turn runs and answers.
            os.chmod(root, 0o555)
            try:
                k3 = cx.kernel(tag="kernel-3", extra_args=["--provider", "replay", "--replies", str(replies_file(cx, [{"agent": "root", "content": "still here"}], name="replies3.jsonl"))])
                cx.rec.expect(k3.start(), "sw-05-start-3", "the kernel did not start a third time")
                c3 = k3.attach()
                c3.wait(lambda f: f.get("type") == "snapshot", 5)
                c3.user("root", "Are you there?")
                answered = c3.wait_turn("root", "idle", 30) is not None
                evs3, _ = transcript(cx.place, "root")
                said = any(e.get("kind") == "assistant" and "still here" in e.get("text", "") for e in evs3)
                cx.rec.notes["usable_while_blocked"] = {"turn_ended": answered, "answered": said}
                if not (answered and said):
                    # The injector that blocks the rename (a read-only agent folder) also blocks the turn's own
                    # writes (inflight/, jobs/), so this probe cannot separate "blocked migration" from "unwritable
                    # folder". Recorded, not failed: the notice's advice — fix what blocks writes in the folder — is
                    # the same fix for both.
                    cx.rec.notes["usable_while_blocked"]["verdict"] = "unverified: the injector blocks ordinary turns too; a read-only agent folder is unusable regardless of the migration"
                k3.stop()
            finally:
                os.chmod(root, 0o755)
        cx.rec.expect(len(crons) <= 1, "sw-05-cron-doubled", f"the legacy standing cron was migrated {len(crons)} times into subscriptions/ (plan.jsonl left in place: {plan_left}) — it will fire that many times", "arbos-kernel migrate.rs: `let _ = rename(plan.jsonl → .migrated)` then migrate again on the next start")
        cx.rec.expect(len([n for n in inbox if "MIGRATED" in (root / "inbox" / n).read_text(errors="replace")]) <= 1 if inbox else True, "sw-05-task-doubled", f"the pending task was queued more than once: {inbox}")


    @reg("sw-06-migration-cut-leaves-something-a-person-can-finish", tags=("silent-write", "migration"))
    def sw06(cx):
        """An earlier start moved plan.jsonl aside as plan.jsonl.migrating and died before writing any record (the cut case). The next start must not migrate again (no doubled crons) — and it must leave a person something they can act on: a transcript notice that names the kept file and what it is, not only a kernel.log line and a `.migrating` file nobody would recognise."""
        arbos = cx.place / ".arbos"
        root = arbos / "agents" / "root"
        for d in ("pages", "jobs", "subscriptions", "inbox"):
            (root / d).mkdir(parents=True, exist_ok=True)
        (root / "agent.md").write_text(f"name: root\ntitle: \nparent: \npaused: false\nmodel: inherit\nallowlist: ls, read, bash, plan, say\nreadonly: false\ncwd: {cx.place}\n")
        (root / "transcript.jsonl").touch()
        now = now_ms()
        rows = [{"id": 1, "parent": 0, "seq": 0, "goal": "tick every 30s", "when": {"every_ms": 30_000, "next_due_ms": now + 3_600_000}, "do": {"kind": "shell", "cmd": "echo legacy-tick >> ticks.txt"}, "status": "pending", "origin": "user", "created_ms": now, "updated_ms": now}]
        # The cut: moved aside, nothing written after.
        (root / "plan.jsonl.migrating").write_text("".join(json.dumps(r) + "\n" for r in rows))
        k = cx.kernel(extra_args=["--provider", "replay", "--replies", str(replies_file(cx, [{"agent": "root", "content": "hi"}]))])
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        time.sleep(2.5)
        c = k.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        c.user("root", "hello")
        c.wait_turn("root", "idle", 30)
        k.stop()
        crons = [p_ for p_ in (root / "subscriptions").glob("*.toml") if "legacy-tick" in p_.read_text(errors="replace")]
        kept = sorted(p_.name for p_ in root.glob("plan.jsonl*"))
        evs, _ = transcript(cx.place, "root")
        notices = [e.get("text", "") for e in evs if e.get("kind") == "notice"]
        told = next((n for n in notices if "migrat" in n.lower() or ".migrating" in n), None)
        log = ""
        for pth in (arbos / "runtime" / "kernel.log", arbos / "kernel.log"):
            if pth.exists():
                log = pth.read_text(errors="replace")
        cx.rec.notes.update({"cron_copies": len(crons), "kept_files": kept, "transcript_notice": (told or "")[:300], "kernel_log_says": '"migrate_cut"' in log})
        # Two honest outcomes for a cut migration: finished once (the standing cron exists exactly once) with the
        # person told, or left with its source kept and the person told. Doubling, or silence, fails.
        cx.rec.expect(len(crons) <= 1, "sw-06-cut-migrated-again", f"a cut migration doubled its work: {len(crons)} cron file(s)", "arbos-kernel migrate.rs Claim::Cut")
        cx.rec.expect(len(crons) == 1 or "plan.jsonl.migrating" in kept, "sw-06-work-lost-and-source-gone", f"the cut migration neither finished (crons={len(crons)}) nor kept its source for a person: {kept}")
        cx.rec.expect(told is not None and ("plan.jsonl" in told or ".migrating" in told), "sw-06-person-not-told", f"the cut is only in kernel.log ({'yes' if '\"migrate_cut\"' in log else 'no'}); the transcript says nothing a person could act on about the old plan", "arbos-kernel migrate.rs Claim::Cut: a notice beside the log line")
        cx.rec.notes["outcome"] = "finished-once-and-told" if (len(crons) == 1 and told) else ("kept-and-told" if told else "silent")


    # ── after something went wrong: the states the rigs test least ────────
    @reg("af-01-folder-renamed-under-a-running-kernel", tags=("after-failure", "filesystem"))
    def af01(cx):
        """The kernel serves `place`; the user renames the folder (Finder, `mv`, a sync tool). Then a line arrives. User-visible questions: does the turn run; where does the transcript go — into the renamed folder, or into a ghost `.arbos/` recreated at the old path; does the window (a fresh attach) see the same history; and does the kernel say anything? A ghost at the old path is the destructive outcome: the user's history splits in two."""
        place = cx.place
        lines = [{"agent": "root", "content": "one"}, {"agent": "root", "content": "two after the move"}]
        k = cx.kernel(extra_args=["--provider", "replay", "--replies", str(replies_file(cx, lines))])
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        c = k.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        c.user("root", "first")
        cx.rec.expect(c.wait_turn("root", "idle", 60) is not None, "af-01-turn-one-never-ended", "turn one never ended")
        before = len(transcript(place, "root")[0])
        moved = place.parent / (place.name + "-renamed")
        os.rename(place, moved)
        time.sleep(1)
        c.user("root", "second")
        ended = c.wait_turn("root", "idle", 60)
        time.sleep(1)
        ghost = (place / ".arbos").exists()
        ghost_lines = len(transcript(place, "root")[0]) if ghost else 0
        moved_lines = len(transcript(moved, "root")[0]) if (moved / ".arbos").exists() else 0
        evs_new = transcript(moved, "root")[0] if (moved / ".arbos").exists() else []
        second_in_moved = any(e.get("kind") == "user" and e.get("text") == "second" for e in evs_new)
        ghost_evs = transcript(place, "root")[0] if ghost else []
        second_in_ghost = any(e.get("kind") == "user" and e.get("text") == "second" for e in ghost_evs)
        err = k.stderr_text()
        said = [l for l in err.splitlines() if "renamed" in l.lower() or "moved" in l.lower() or "no such" in l.lower() or "not found" in l.lower()][-3:]
        notices = [e.get("text", "")[:160] for e in evs_new + ghost_evs if e.get("kind") == "notice"]
        cx.rec.notes.update({"turn_after_move_ended": ended is not None, "lines_before": before, "moved_folder_lines": moved_lines, "ghost_at_old_path": ghost, "ghost_lines": ghost_lines, "second_landed_in": ("moved" if second_in_moved else "") + ("ghost" if second_in_ghost else "") or "nowhere", "kernel_stderr_says": said, "notices": notices[-2:]})
        cx.rec.expect(not ghost, "af-01-ghost-folder-at-old-path", f"after the folder was renamed, the kernel recreated `.arbos/` at the old path ({ghost_lines} transcript lines there; {moved_lines} in the renamed folder): the user's history is now in two places", "arbos-kernel serve/turn: paths held absolute at start; the place moved under it")
        kernel_exited = not k.alive() or any("store is gone" in l for l in said)
        cx.rec.notes["kernel_exited_on_purpose"] = kernel_exited
        # #377: a kernel whose .arbos is gone from under it stops itself and says so on stderr — no ghost. The turn
        # then cannot end; that is the designed outcome, not a hang. What the user still loses: the line typed after
        # the rename, and no word of it reaches the transcript they will reopen.
        cx.rec.expect(ended is not None or kernel_exited, "af-01-turn-hung-after-move", "the turn after the rename neither ended nor did the kernel stop itself")
        cx.rec.expect(second_in_moved or bool(notices), "af-01-line-lost-in-silence", f"the line typed after the rename went {cx.rec.notes['second_landed_in']}; the kernel {'stopped itself (stderr only)' if kernel_exited else 'kept running'} and nothing a person could read says the folder moved or the line was dropped", "arbos-kernel serve: on a lost store, a last notice into the moved folder's transcript, or the desktop's respawn telling the user")
        # put it back so teardown finds it
        try:
            k.kill()
            if moved.exists() and not place.exists():
                os.rename(moved, place)
        except Exception:  # noqa: BLE001
            pass

    @reg("af-02-two-windows-on-one-place", needs_model=True, tags=("after-failure", "desktop"))
    def af02(cx):
        """Two desktop windows (two app processes) open on the same place. Expected: one kernel serves both (the second attaches, no second spawn); a line typed in either appears exactly once in both; a Stop pressed in one ends the turn in the other. The destructive outcome is a second kernel on the same folder, or a line delivered twice."""
        if not desktop_available():
            cx.rec.notes["skipped"] = "desktop binary/driver/Xvfb missing"
            return
        from journey_scenarios import Rig, kernel_pid, read_transcript

        folder = cx.scratch / "shared-project"
        folder.mkdir(parents=True, exist_ok=True)
        a = Rig(cx, [folder], tag="app-a")
        b = None
        try:
            time.sleep(3)
            a.focus(folder)
            a.wait_idle(folder, 60)
            pid_a = kernel_pid(folder)
            # A second app process on the same place, its own xdg so it is a second window, not the same state.
            saved = cx.scratch / "xdg"
            second = cx.scratch / "xdg-b"
            shutil.copytree(saved, second, dirs_exist_ok=True)
            b = Rig(cx, [folder], tag="app-b")
            time.sleep(3)
            b.focus(folder)
            pid_b = kernel_pid(folder)
            kernels = [l for l in subprocess.run(["ps", "-eo", "pid=,args="], capture_output=True, text=True).stdout.splitlines() if str(folder) in l and " serve " in l]
            tag = f"TWO-{now_ms() % 100000}"
            a.send(f"Reply with the single word {tag}.")
            a.wait_busy(folder, 20)
            a.wait_idle(folder, 90)
            time.sleep(2)
            evs = read_transcript(folder)
            user_lines = [e for e in evs if e.get("kind") == "user" and tag in e.get("text", "")]
            items_a = [i for i in (a.root_chat(folder) or {}).get("items", []) if tag in str(i.get("text", ""))]
            items_b = [i for i in (b.root_chat(folder) or {}).get("items", []) if tag in str(i.get("text", ""))]
            # Stop from B while A runs a slow turn.
            a.send("Run `sleep 40; echo slow` with bash, then say done.")
            a.wait_busy(folder, 20)
            time.sleep(3)
            try:
                b.app.click("composer-stop")
                b.pulse("Stop from the second window")
                stopped_from_b = True
            except Exception as e:  # noqa: BLE001
                stopped_from_b = False
                cx.rec.notes["stop_error"] = str(e)[:120]
            ended = a.wait_idle(folder, 20)
            evs2 = read_transcript(folder)
            interrupted = any(e.get("kind") == "interrupted" for e in evs2[len(evs):])
            cx.rec.notes.update({"kernel_pid_a": pid_a, "kernel_pid_b": pid_b, "kernels_serving_folder": len(kernels), "line_on_transcript": len(user_lines), "shown_in_a": len(items_a), "shown_in_b": len(items_b), "stop_from_b": stopped_from_b, "a_ended_after_b_stop": ended, "interrupted": interrupted})
            cx.rec.expect(len(kernels) == 1 and pid_a == pid_b, "af-02-second-kernel", f"{len(kernels)} kernel(s) serving one folder (pids {pid_a} / {pid_b}) — two windows spawned two kernels", "desktop kernel.rs attach_or_spawn / kernel lock")
            cx.rec.expect(len(user_lines) == 1, "af-02-line-delivered-twice", f"the line typed once is on the transcript {len(user_lines)} time(s)")
            cx.rec.expect(len(items_a) >= 1 and len(items_b) >= 1, "af-02-other-window-blind", f"the line shows in A {len(items_a)}x and in B {len(items_b)}x — a window on the same place did not see it")
            if stopped_from_b:
                cx.rec.expect(ended and interrupted, "af-02-stop-not-shared", "Stop pressed in the second window did not end the turn the first window started")
        finally:
            if b:
                b.close(folders=[])
            a.close(folders=[folder])
            cx.rec.snapshot(folder, "shared-after")


    @reg("af-03-desktop-folder-renamed-under-the-window", needs_model=True, tags=("after-failure", "desktop", "filesystem"))
    def af03(cx):
        """The window is open on a place; the folder is renamed on disk; the user types a line. The kernel stops itself (#377). What does the window do — respawn a kernel at the OLD path and mint a ghost `.arbos/` there (destructive: history split), tell the user the folder moved, or follow it? Recorded as the user sees it: the chat items, a ghost folder, and where the new line landed."""
        if not desktop_available():
            cx.rec.notes["skipped"] = "desktop binary/driver/Xvfb missing"
            return
        from journey_scenarios import Rig, kernel_pid, read_transcript

        folder = cx.scratch / "moving-project"
        folder.mkdir(parents=True, exist_ok=True)
        moved = cx.scratch / "moving-project-renamed"
        rig = Rig(cx, [folder], tag="app-move")
        try:
            time.sleep(3)
            rig.focus(folder)
            rig.wait_idle(folder, 60)
            pid0 = kernel_pid(folder)
            items0 = len((rig.root_chat(folder) or {}).get("items", []))
            os.rename(folder, moved)
            time.sleep(4)
            tag = f"MOVED-{now_ms() % 100000}"
            rig.send(f"Reply with the single word {tag}.")
            rig.pulse("typing after the folder was renamed")
            end = time.time() + 45
            while time.time() < end:
                if (folder / ".arbos").exists() or any(e.get("kind") == "user" and tag in e.get("text", "") for e in read_transcript(moved)):
                    break
                time.sleep(1)
            time.sleep(3)
            ghost = (folder / ".arbos").exists()
            in_moved = any(e.get("kind") == "user" and tag in e.get("text", "") for e in read_transcript(moved))
            in_ghost = ghost and any(e.get("kind") == "user" and tag in e.get("text", "") for e in read_transcript(folder))
            chat = rig.root_chat(folder) or rig.root_chat(moved) or {}
            items = chat.get("items", [])[items0:]
            notices = [str(i.get("text", ""))[:160] for i in items if i.get("kind") == "notice"]
            pid1 = kernel_pid(folder) if ghost else kernel_pid(moved)
            cx.rec.notes.update({"kernel_pid_before": pid0, "kernel_pid_after": pid1, "ghost_at_old_path": ghost, "line_in_moved": in_moved, "line_in_ghost": in_ghost, "connection": chat.get("connection"), "new_items": [(i.get("kind"), str(i.get("text", ""))[:80]) for i in items][:6]})
            cx.rec.expect(not ghost, "af-03-ghost-folder-minted", f"after the rename the window respawned a kernel at the old path and minted a fresh `.arbos/` there (line landed in ghost: {in_ghost}); the user's history is now split between {folder.name} and {moved.name}", "desktop kernel.rs attach_or_spawn on a path whose folder is gone")
            cx.rec.expect(bool(notices) or in_moved, "af-03-silent", f"the folder moved under the window and the chat says nothing ({cx.rec.notes['new_items']}); the line went {'to the moved folder' if in_moved else 'nowhere'}")
            # What the notice says must be true: the folder moved; nothing was archived.
            wrong = [n for n in notices if "archived" in n.lower() and not any(w in n.lower() for w in ("moved", "renamed", "folder", "no longer", "not found"))]
            # `wrong[0]` inside the message is evaluated before `expect` looks at the condition, so the
            # passing case — `wrong` empty — raised `IndexError` and the scenario reported
            # `driver-exception` in every cycle from 2026-09-17 18:18 to 2026-09-18 07:06 while the product
            # was behaving: no ghost folder, and an honest "folder is gone or was moved" notice.
            explained = repr(wrong[0]) if wrong else "nothing"
            cx.rec.expect(not wrong, "af-03-wrong-explanation", f"the chat explains a renamed folder as {explained} — nothing was archived; the user's line went {'to the moved folder' if in_moved else 'nowhere'}", "desktop session.rs: a kernel that stopped because its store is gone is drawn as an archived agent")
        finally:
            try:
                rig.close(folders=[folder, moved])
            except Exception:  # noqa: BLE001
                pass
            for pth in (folder, moved):
                if pth.exists():
                    cx.rec.snapshot(pth, f"{pth.name}-after")


    # ── the standing_pass_e2e race: a rewind racing its own turn's checkpoint ──
    @reg("rw-06-rewind-right-after-sending-cuts-one-turn-too-many", tags=("rewind", "history", "race"))
    def rw06(cx):
        """Two turns done. Send a third message and, within milliseconds, rewind to turn 3 (files: false — the record only). Expected every time: the transcript keeps turns 1 and 2 whole (their user lines and replies) and drops only the third. The race (#398): turn 3's checkpoint is written un-awaited on the blocking pool, so a rewind arriving first resolves 'turn 3' to turn 2's checkpoint and cuts turn 2 as well — the message before the one you rewound is gone; if it was the first, the record is empty. Run pinned to one core beside four spinners, 12 attempts, reported as a rate."""
        place = cx.place
        user_repo(place)
        attempts = int(os.environ.get("ARBOS_QA_RW06_ATTEMPTS", "12"))
        replies = []
        for i in range(attempts * 3 + 6):
            replies.append({"agent": "root", "content": f"reply {i}"})
        spinners = [subprocess.Popen(["taskset", "-c", "0", "sh", "-c", "while :; do :; done"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL) for _ in range(4)]

        def pin():
            os.sched_setaffinity(0, {0})
        try:
            k = cx.kernel(preexec=pin, extra_args=["--provider", "replay", "--replies", str(replies_file(cx, replies))])
            cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
            c = k.attach()
            c.wait(lambda f: f.get("type") == "snapshot", 5)
            results = []
            for n in range(attempts):
                # two turns that must survive
                a, b, third = f"keep-a-{n}", f"keep-b-{n}", f"drop-{n}"
                for t in (a, b):
                    c.user("root", t)
                    if c.wait_turn("root", "idle", 60) is None:
                        results.append({"attempt": n, "outcome": "setup-turn-never-ended"}); break
                else:
                    evs_before, _ = transcript(place, "root")
                    turns_before = sum(1 for e in evs_before if e.get("kind") == "user")
                    # the third message and the rewind to it, back to back
                    c.user("root", third)
                    c.send({"type": "rewind", "agent": "root", "turn": turns_before + 1, "files": False})
                    first = c.wait(lambda f: f.get("type") == "rewound" and f.get("agent") == "root", 20, "rewound")
                    end = time.time() + 15
                    settled = None
                    while time.time() < end:
                        evs, _ = transcript(place, "root")
                        if evs and evs[-1].get("kind") == "turn_complete" and not any(e.get("kind") == "user" and e.get("text") == third for e in evs):
                            settled = evs; break
                        time.sleep(0.1)
                    evs = settled or transcript(place, "root")[0]
                    users = [e.get("text") for e in evs if e.get("kind") == "user"]
                    outcome = "exact" if (a in users and b in users and third not in users) else ("one-too-many" if (a in users and b not in users) else ("blank" if not users else ("third-kept" if third in users else "other")))
                    results.append({"attempt": n, "outcome": outcome, "users_kept": len(users), "rewound": first is not None, "settled": settled is not None})
                    if outcome in ("blank",):
                        break
            k.stop()
        finally:
            for sp in spinners:
                sp.kill()
        counts = {}
        for r in results:
            counts[r["outcome"]] = counts.get(r["outcome"], 0) + 1
        cx.rec.notes.update({"attempts": len(results), "outcomes": counts, "results": results[:12]})
        bad = sum(v for k_, v in counts.items() if k_ in ("one-too-many", "blank"))
        cx.rec.expect(bad == 0, "rw-06-cut-one-turn-too-many", f"in {bad} of {len(results)} rewinds sent right after a message, the message before it was cut too ({counts}) — a person pressing Rewind after sending, on a busy machine, loses a turn from the record", "arbos-engine turn.rs snapshot_turn un-awaited vs rewind resolving turn N (#398)")
        cx.rec.expect(counts.get("third-kept", 0) == 0, "rw-06-rewind-did-nothing", f"{counts.get('third-kept', 0)} rewind(s) left the third message in place")


    # ── the start-time exclude: a repository that appears after the kernel started ──
    def rw07_body(cx, how):
        """`how`: 'init-in-turn' — a fresh folder, kernel first, `git init` by the agent in turn one; 'reinit' — a repo
        deleted and recreated mid-session; 'clone' — the folder gains a .git from a clone during a turn."""
        place = cx.place
        root = place / ".arbos" / "agents" / "root"
        (root / "pages").mkdir(parents=True, exist_ok=True)
        (root / "jobs").mkdir(exist_ok=True)
        (root / "agent.md").write_text(f"name: root\ntitle: \nparent: \npaused: false\nmodel: inherit\nallowlist: ls, read, write, edit, bash, undo, changes, plan, say\nreadonly: false\ncwd: {place}\n")
        (root / "transcript.jsonl").touch()
        (place / ".arbos" / "project.toml").write_text('schema = 2\n\n[git]\nprotected = []\n')
        gitid = "git -c user.name=qa -c user.email=qa@qa"
        if how == "reinit":
            subprocess.run(["git", "init", "-q", "-b", "work"], cwd=place, capture_output=True)  # not a protected branch name
            subprocess.run(["git", "-c", "user.name=qa", "-c", "user.email=qa@qa", "commit", "-q", "--allow-empty", "-m", "start"], cwd=place, capture_output=True)
        if how == "clone":
            src = cx.scratch / "upstream"
            src.mkdir()
            subprocess.run(["git", "init", "-q"], cwd=src, capture_output=True)
            (src / "README.md").write_text("upstream\n")
            subprocess.run(["git", "add", "-A"], cwd=src, capture_output=True)
            subprocess.run(["git", "-c", "user.name=qa", "-c", "user.email=qa@qa", "commit", "-q", "-m", "up"], cwd=src, capture_output=True)
        first = {
            "init-in-turn": f"git init -q -b work && {gitid} commit -q --allow-empty -m start && echo one > f1.txt",
            "reinit": f"rm -rf .git && git init -q -b work && {gitid} commit -q --allow-empty -m start && echo one > f1.txt",
            "clone": f"git clone -q {cx.scratch / 'upstream'} tmpclone && mv tmpclone/.git .git && rm -rf tmpclone && git reset -q && echo one > f1.txt",
        }[how]
        lines = [
            {"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": first, "description": "turn one: the repository appears"}}]},
            {"agent": "root", "content": "first"},
            {"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": "echo two > f2.txt", "description": "f2"}}]},
            {"agent": "root", "content": "second"},
            {"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": "echo three > f3.txt", "description": "f3"}}]},
            {"agent": "root", "content": "third"},
        ]
        k = cx.kernel(extra_args=["--provider", "replay", "--replies", str(replies_file(cx, lines))])
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        c = k.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        for t in ("one", "two", "three"):
            c.user("root", t)
            cx.rec.expect(c.wait_turn("root", "idle", 60) is not None, f"rw-07-{how}-turn-never-ended", f"turn {t!r} never ended")
        time.sleep(1.5)
        cps_path = root / "checkpoints.jsonl"
        cps = cps_path.read_text().splitlines() if cps_path.exists() else []
        sidecar = sorted(p_.name for p_ in (root / "checkpoints.d").glob("*.json")) if (root / "checkpoints.d").exists() else []
        with_tree = sum(1 for l in cps if '"work"' in l) + sum(1 for n in sidecar if '"work"' in (root / "checkpoints.d" / n).read_text(errors="replace"))
        files_before = sorted(p_.name for p_ in place.glob("f*.txt"))
        exclude = (place / ".git" / "info" / "exclude").read_text(errors="replace") if (place / ".git" / "info" / "exclude").exists() else ""
        c.send({"type": "rewind", "agent": "root", "turn": 3, "files": True})
        c.wait(lambda f: f.get("type") == "rewound" and f.get("agent") == "root", 20, "rewound")
        follow = c.wait(lambda f: (f.get("type") == "rewound" and f.get("restored") is not None) or f.get("type") == "error", 40, "the restore's report")
        time.sleep(1)
        files_after = sorted(p_.name for p_ in place.glob("f*.txt"))
        evs, _ = transcript(place, "root")
        cx.rec.notes.update({"how": how, "checkpoints": len(cps), "checkpoints_with_tree": with_tree, "sidecar": sidecar[:4], "arbos_excluded_at_rewind": ".arbos" in exclude, "files_before": files_before, "files_after": files_after, "follow": {k_: str(v)[:200] for k_, v in (follow or {}).items()}, "transcript_users": [e.get("text") for e in evs if e.get("kind") == "user"]})
        kept_ok = "f1.txt" in files_after and "f2.txt" in files_after
        cx.rec.expect(kept_ok, f"rw-07-{how}-kept-turns-files-lost", f"after rewinding to turn 3 the kept turns' files are {files_after} (expected f1, f2; checkpoints with a saved tree: {with_tree}/{len(cps)}; .arbos excluded: {'.arbos' in exclude}); the restore said {cx.rec.notes['follow']}", "arbos-engine tools/git.rs work_commit: `git add -A` refuses the embedded .arbos repo when the start-time exclude is missing (#398 adds its own excludes file)")
        cx.rec.expect("f3.txt" not in files_after or (follow or {}).get("type") == "error", f"rw-07-{how}-third-turn-file-kept", f"turn 3's file survived a rewind with files: {files_after} — the restore did not happen and did not say so")
        cx.rec.expect(with_tree >= 1, f"rw-07-{how}-no-tree-in-any-checkpoint", f"none of the {len(cps)} checkpoints of this session saved the working tree — the start-time exclude of .arbos/ was missing and every tree save was refused")

    def rw07(how, doc):
        @reg(f"rw-07-{how}-repo-after-kernel-start-rewind-keeps-files", tags=("rewind", "history", "fresh-place"))
        def fn(cx):
            rw07_body(cx, how)
        fn.__doc__ = doc
        return fn

    rw07("init-in-turn", "A fresh folder; the kernel starts; the agent runs `git init` in turn one and writes f1; turns two and three write f2, f3. Rewind to turn 3 with files: f1 and f2 must survive. Without the start-time exclude of .arbos/, `git add -A` refused every tree save of the session — the checkpoint had no tree and the rewind deleted the kept turns' files (pre-#390) or refused (post); #398 excludes .arbos/ on its own.")
    rw07("reinit", "The same, for a repository deleted (`rm -rf .git`) and recreated in turn one of a session that started on a repository.")
    rw07("clone", "The same, for a folder that gains its .git from a clone during turn one.")

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

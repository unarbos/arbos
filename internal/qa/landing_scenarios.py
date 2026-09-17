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
            rig.wait_idle(folder, 60)
            rig.send("Run `sleep 40; echo slow` with bash, then say done.")
            cx.rec.expect(rig.wait_busy(folder, 20), "sq-02-turn-never-started", "the slow turn never started")
            time.sleep(3)
            # Enter during a turn steers (#362 makes the command yield to it); the QUEUED follow-up is
            # cmd/ctrl-shift-enter — "queue next" — the row under the composer that Stop used to delete.
            inbox = folder / ".arbos" / "agents" / "root" / "inbox"
            rig.app.wait_element("composer-field", reachable=True)
            rig.app.click("composer-field")
            rig.app.type("Then reply with the single word FOLLOWUP.")
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
        return {
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
            cx.rec.expect(not wrong, "af-03-wrong-explanation", f"the chat explains a renamed folder as {wrong[0]!r} — nothing was archived; the user's line went {'to the moved folder' if in_moved else 'nowhere'}", "desktop session.rs: a kernel that stopped because its store is gone is drawn as an archived agent")
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

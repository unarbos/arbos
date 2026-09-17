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

"""#444, the unchecked-write pass: the four destructive readers, each driven against a control that fails.

The audit (`internal/unchecked-writes-and-orderings-audit.md`, 122 sites at `main` 7017eb75) asked two
questions of every `let _ = write`: what does the next reader believe if this did not land, and what does
it read if it landed somewhere else. Four answers were "it acts on a record that is not there". #444
fixes those four. These probes make the write fail *before any record exists* — the `lk-02` shape, so a
pass proves the record was kept rather than that it happened to be readable — and then ask what the next
reader believes.

  uw-01  the undo mark (`runtime/checkpoint`, engine `tools/git.rs`). Three ways the write can fail,
         because the fix is `remove_file` on failure and a removal needs the *parent* writable:
         (a) no mark has ever existed and `runtime/` is read-only — `undo` must refuse and reset nothing;
         (b) a stale mark from turn one, the mark file unwritable, the parent writable — the fix removes
         it and `undo` refuses, where the control resets to turn one's HEAD and destroys turn two's
         commit; (c) the same with the parent read-only, which is what a read-only mount gives you and
         where the fix's own removal cannot land either.
  uw-02  `.arbos/` into `.git/info/exclude` (core `files.rs`). `.git/info` is made read-only before the
         first start, so the line was never written and the person's next `git add -A` stages the agent's
         whole record. The kernel cannot stop that; it can say so. One failed notice per start naming the
         remedy, said once rather than once a turn — and the harm is recorded on both builds, so the
         report says what is still true after the fix.
  uw-03  the panic path's unrecorded end (kernel `sched.rs`). The turn panics (ARBOS_TEST_PANIC_TURN)
         while root's transcript cannot be appended to. The control leaves no notice, no `turn_complete`
         and no line anywhere; the fix logs `turn_panicked_unrecorded` and sends the words to the
         attached client live.
  uw-04  a subscription job's marker (kernel `subs.rs`), written after the process has started. Unwritten,
         a restart finds a job it cannot place and the run's result reaches nobody. The window between
         the job folder's creation and the marker write is narrow, so this one races for it and reports
         the rate; never caught is a skip, not a pass.

Every arm names the build it was taken on and was read against a control that fails. The rollout's
`result.json` → `notes` holds what each measured.
"""

import datetime
import json
import os
import resource
import shutil
import signal
import subprocess
import threading
import time
from pathlib import Path

import desktop_scenarios
import fileplan_scenarios


def replies_file(cx, lines, name="replies.jsonl"):
    p = cx.scratch / name
    p.write_text("".join(json.dumps(l) + "\n" for l in lines))
    return p


def kernel_rows(place):
    """The structured kernel log, parsed. Not grepped as text: numbers in a log are clocks and sizes, and
    only prose should be searched for prose (#335)."""
    p = Path(place) / ".arbos" / "runtime" / "kernel.log"
    rows = []
    if p.exists():
        for line in p.read_text(errors="replace").splitlines():
            if line.strip():
                try:
                    rows.append(json.loads(line))
                except json.JSONDecodeError:
                    pass
    return rows


def git_out(place, *args):
    return subprocess.run(["git", *args], cwd=str(place), capture_output=True, text=True).stdout.strip()


def plain_agent(place, allowlist="ls, read, write, edit, bash, undo, changes, plan, say"):
    """root as a plain agent: a coordinator has neither `undo` nor project writes."""
    root = Path(place) / ".arbos" / "agents" / "root"
    (root / "pages").mkdir(parents=True, exist_ok=True)
    (root / "jobs").mkdir(exist_ok=True)
    (root / "agent.md").write_text(
        f"name: root\ntitle: \nparent: \npaused: false\nmodel: inherit\n"
        f"allowlist: {allowlist}\nreadonly: false\ncwd: {place}\n"
    )
    (root / "transcript.jsonl").touch()
    (Path(place) / ".arbos" / "project.toml").write_text('schema = 2\n\n[git]\nprotected = []\n')
    return root


def user_repo(place):
    """The project is a git repository with an identity, on a branch that is not protected."""
    for args in (["init", "-q"], ["commit", "-q", "--allow-empty", "-m", "start"]):
        subprocess.run(["git", "-c", "user.name=qa", "-c", "user.email=qa@qa", *args], cwd=str(place), capture_output=True)
    subprocess.run(["git", "config", "user.name", "qa"], cwd=str(place), capture_output=True)
    subprocess.run(["git", "config", "user.email", "qa@qa"], cwd=str(place), capture_output=True)
    subprocess.run(["git", "checkout", "-q", "-b", "work"], cwd=str(place), capture_output=True)


def wait_turn_complete(c, agent, timeout):
    """The turn's own end, not `idle`. The `turn` frame's `idle` state arrives only after the notes nudge
    that follows a turn, so it can trail the fact by seconds — and a scenario that waits for `idle` and
    then reads the transcript can read before the turn's end has landed on disk. Learned on the kernel
    side 2026-09-17: two frames that both seem to mean "the turn is over" differ by seconds, and the one
    that sounds definitive is the later, looser one. Wait on the fact the assertion reads."""
    return c.wait(
        lambda f: f.get("type") == "event"
        and f.get("agent") == agent
        and (f.get("event") or {}).get("kind") == "turn_complete",
        timeout,
        f"turn_complete event for {agent}",
    )


def register(scenario, registry, transcript, now_ms, branch):
    def reg(name, needs_model=False, tags=()):
        def deco(fn):
            scenario(name, needs_model=needs_model, tags=("landing", "unchecked-write") + tuple(tags))(fn)
            registry[name]["branch"] = branch
            return fn

        return deco

    def tool_said(evs, name):
        return [
            (e.get("error") or str(e.get("body") or e.get("result") or ""))[:200]
            for e in evs
            if e.get("kind") == "tool" and e.get("name") == name
        ]

    def notices(evs):
        return [e for e in evs if e.get("kind") == "notice"]

    COMMIT_A = {"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": "echo A > a.txt && git add a.txt && git commit -q -m 'turn one: A' && git rev-parse HEAD", "description": "commit A"}}]}
    COMMIT_B = {"agent": "root", "content": "", "calls": [{"name": "bash", "arguments": {"command": "echo B > b.txt && git add b.txt && git commit -q -m 'turn two: B' && git rev-parse HEAD", "description": "commit B"}}]}
    CALL_UNDO = {"agent": "root", "content": "", "calls": [{"name": "undo", "arguments": {}}]}
    SAID_SO = {"agent": "root", "content": "Done."}

    # ── #444: the undo mark ────────────────────────────────────────────────
    @reg("uw-01-unwritable-undo-mark-refuses-rather-than-using-an-older-turns", tags=("undo", "destructive-order"))
    def uw01(cx):
        """A mark that could not be written must not leave `undo` acting on an older turn's mark. Three arms,
        each a fresh kernel: (a) no mark has ever existed and `runtime/` is read-only; (b) a stale mark from
        turn one with the mark file unwritable and its folder writable; (c) the same with the folder read-only,
        where the fix's own `remove_file` cannot land. qal-j10's shape, #444's fix."""
        place = cx.place
        place.mkdir(parents=True, exist_ok=True)
        user_repo(place)
        (place / ".gitignore").write_text(".arbos/\n")
        subprocess.run(["git", "add", ".gitignore"], cwd=str(place), capture_output=True)
        subprocess.run(["git", "commit", "-q", "-m", "ignore"], cwd=str(place), capture_output=True)
        plain_agent(place)
        head0 = git_out(place, "rev-parse", "HEAD")
        mark = place / ".arbos" / "runtime" / "checkpoint"
        runtime = mark.parent

        def back_to_start():
            subprocess.run(["git", "reset", "-q", "--hard", head0], cwd=str(place), capture_output=True)
            subprocess.run(["git", "clean", "-qfd", "-e", ".arbos"], cwd=str(place), capture_output=True)
            mark.unlink(missing_ok=True)

        def no_mark_arm():
            # Before any record exists, and unwritable from the start: the mark is an empty file nothing can
            # write to, so turn one's own start cannot record its HEAD. (A read-only `runtime/` folder is not
            # a world this bug lives in — the kernel exits 1 at start when it cannot write that folder, so
            # there is no turn to measure. Measured, not assumed: 2026-09-17 12:51 on cbbe9922d6a2.)
            runtime.mkdir(parents=True, exist_ok=True)
            mark.write_text("")
            os.chmod(mark, 0o444)
            k = cx.kernel(tag="a-no-mark", extra_args=["--provider", "replay", "--replies", str(replies_file(cx, [COMMIT_B, CALL_UNDO, SAID_SO], "replies-a.jsonl"))])
            try:
                if not k.start():
                    return None
                c = k.attach()
                c.wait(lambda f: f.get("type") == "snapshot", 5)
                c.user("root", "Commit B, then undo this turn.")
                ended = wait_turn_complete(c, "root", 90) is not None
                evs, _ = transcript(place, "root")
                head = git_out(place, "rev-parse", "HEAD")
                return {
                    "turn_ended": ended,
                    "head_before": head0[:12],
                    "head_after_undo": head[:12],
                    "b_committed": head != head0,
                    "mark_exists_after": mark.exists(),
                    "undo_said": tool_said(evs, "undo")[-1:],
                    # #444 makes the checkpoint step return an error rather than swallow it. Who reads that?
                    # If an unwritable mark now ends a person's turn, that is a new cost and belongs here.
                    "turn_notices": [e.get("text", "")[:200] for e in notices(evs)][-3:],
                    "turn_kinds": [e.get("kind") for e in evs][-5:],
                    "log_after": git_out(place, "log", "--oneline").splitlines()[:3],
                }
            finally:
                runtime.chmod(0o755)
                if mark.exists():
                    os.chmod(mark, 0o644)
                k.stop()

        def stale_mark_arm(tag, folder_ro):
            k = cx.kernel(tag=tag, extra_args=["--provider", "replay", "--replies", str(replies_file(cx, [COMMIT_A, {"agent": "root", "content": "Committed A."}, COMMIT_B, CALL_UNDO, SAID_SO], f"replies-{tag}.jsonl"))])
            try:
                if not k.start():
                    return None
                c = k.attach()
                c.wait(lambda f: f.get("type") == "snapshot", 5)
                c.user("root", "Commit A.")
                one = wait_turn_complete(c, "root", 90) is not None
                head_a = git_out(place, "rev-parse", "HEAD")
                mark_before = mark.read_text().strip().splitlines()[0] if mark.exists() else None
                # The injector: turn two's start cannot record HEAD=A. A full disk and an unwritable
                # runtime/ are the two real causes; the folder arm is the one where removal fails too.
                if mark.exists():
                    os.chmod(mark, 0o444)
                if folder_ro:
                    runtime.chmod(0o555)
                c.user("root", "Commit B, then undo this turn.")
                two = wait_turn_complete(c, "root", 90) is not None
                if folder_ro:
                    runtime.chmod(0o755)
                if mark.exists():
                    os.chmod(mark, 0o644)
                head_after = git_out(place, "rev-parse", "HEAD")
                evs, _ = transcript(place, "root")
                return {
                    "turn_one_ended": one,
                    "turn_two_ended": two,
                    "head_after_turn_one": head_a[:12],
                    "a_committed": head_a != head0,
                    "mark_before_turn_two": (mark_before or "")[:12],
                    "mark_after_turn_two": (mark.read_text().strip().splitlines()[0][:12] if mark.exists() and mark.read_text().strip() else None),
                    "mark_exists_after": mark.exists(),
                    "head_after_undo": head_after[:12],
                    "a_txt_exists": (place / "a.txt").exists(),
                    "a_reachable_from_head": subprocess.run(["git", "merge-base", "--is-ancestor", head_a, "HEAD"], cwd=str(place), capture_output=True).returncode == 0,
                    "undo_said": tool_said(evs, "undo")[-1:],
                    "log_after": git_out(place, "log", "--oneline").splitlines()[:4],
                }
            finally:
                try:
                    runtime.chmod(0o755)
                    if mark.exists():
                        os.chmod(mark, 0o644)
                except OSError:
                    pass
                k.stop()

        results = {"a_no_mark_runtime_ro": no_mark_arm()}
        back_to_start()
        results["b_stale_mark_file_ro"] = stale_mark_arm("b-stale-file-ro", folder_ro=False)
        back_to_start()
        results["c_stale_mark_folder_ro"] = stale_mark_arm("c-stale-folder-ro", folder_ro=True)
        cx.rec.notes.update({"head0": head0[:12], "results": results})

        a, b, c3 = results["a_no_mark_runtime_ro"], results["b_stale_mark_file_ro"], results["c_stale_mark_folder_ro"]
        if not all(r is not None for r in (a, b, c3)):
            cx.rec.expect(False, "probe-kernel-did-not-start", f"an arm could not start its kernel (a={a is not None}, b={b is not None}, c={c3 is not None}); nothing measured")
            return
        # A pass proves it looked at something first: without turn one's commit the stale arms measure nothing.
        if not (b["a_committed"] and c3["a_committed"]):
            cx.rec.expect(False, "probe-turn-one-did-not-commit", f"turn one's commit did not land (b={b['a_committed']}, c={c3['a_committed']}); the stale-mark arms measured nothing")
            return

        # (a) The mark was unwritable before any record existed. Nothing from before the turn may be lost,
        # the turn must end, and `undo` must either say something or find no mark at all. Both builds are
        # expected to hold here: the arm's worth is that the refusal path destroys nothing, and that it
        # shows what #444's new error from the checkpoint step does to an ordinary turn.
        a["head0_reachable"] = subprocess.run(["git", "merge-base", "--is-ancestor", head0, "HEAD"], cwd=str(place), capture_output=True).returncode == 0
        cx.rec.expect(a["turn_ended"], "uw-01a-turn-never-ended", f"with the mark unwritable from before the first turn, the turn never reached idle: {a}")
        cx.rec.expect(
            bool(a["undo_said"]) or not a["mark_exists_after"],
            "uw-01a-undo-said-nothing-and-the-mark-stayed",
            f"the mark could not be written before any record existed, and afterwards `undo` said nothing while the unusable mark was still in place: {a}",
            "arbos-engine tools/git.rs — remove a mark that could not be written, and refuse with a reason (#444)",
        )

        # Did the injection actually stop this turn's mark being written? Since #392 the mark is written
        # with `write_atomic` (temp file + rename), which replaces an unwritable *file* without needing to
        # write it — so arms (a) and (b) no longer stage the fault on a build that has #392, and a pass
        # there says nothing about the unwritten-mark path. Only an unwritable *folder* (arm c) defeats a
        # rename as well. Each arm says which of the two it did.
        # Staged means: after turn two's start the mark does NOT name that turn's starting HEAD, which is
        # turn one's commit. Comparing with the mark's *previous* value instead would read #444's removal
        # (mark gone, so the value changed) as a successful write, and call a staged arm unstaged.
        for tag, r in (("b", b), ("c", c3)):
            r["staged_the_fault"] = r["mark_after_turn_two"] != r["head_after_turn_one"]
        # Arm (a) has no earlier mark to compare against, so it is read from what `undo` found: a refusal
        # means no usable mark was there, which is what the injection was for.
        a["staged_the_fault"] = "no checkpoint" in " ".join(a["undo_said"]).lower()
        staged_arms = [t for t, r in (("a", a), ("b", b), ("c", c3)) if r.get("staged_the_fault")]
        cx.rec.notes["arms_that_staged_the_fault"] = staged_arms
        cx.rec.expect(
            bool(staged_arms),
            "probe-no-arm-staged-an-unwritten-mark",
            f"no arm stopped the mark being written for its turn (marks before/after: a={a['mark_exists_after']}, b={b['mark_before_turn_two']}->{b['mark_after_turn_two']}, c={c3['mark_before_turn_two']}->{c3['mark_after_turn_two']}); on this build the injections are defeated and the run proves nothing about an unwritten mark",
        )

        for tag, r in (("b", b), ("c", c3)):
            if not r["staged_the_fault"]:
                r["outcome"] = "arm-did-not-stage-the-fault: the mark was written for this turn anyway (atomic rename over an unwritable file, #392)"
                continue
            refused = any(("no checkpoint" in s.lower() or "nothing reset" in s.lower() or "refus" in s.lower() or "could not" in s.lower()) for s in r["undo_said"])
            r["refused"] = refused
            r["outcome"] = (
                "restored-to-turn-start" if r["head_after_undo"] == r["head_after_turn_one"]
                else ("refused-with-a-reason" if refused and r["a_reachable_from_head"] else "destroyed-committed-work")
            )
            cx.rec.expect(
                r["a_reachable_from_head"] and r["a_txt_exists"],
                f"uw-01{tag}-undo-destroyed-committed-work",
                f"arm ({tag}, mark folder {'read-only' if tag == 'c' else 'writable'}): the mark could not be rewritten at turn two's start, and `undo` reset to {r['head_after_undo']} — turn one's commit {r['head_after_turn_one']} is unreachable and a.txt is {'there' if r['a_txt_exists'] else 'gone'}. The mark said {r['mark_before_turn_two']}; `undo` reported {r['undo_said']}",
                "arbos-engine tools/git.rs snapshot_turn_tree — a mark that cannot be written is removed, and `undo` with no mark refuses (#444)",
            )
            cx.rec.expect(
                r["head_after_undo"] == r["head_after_turn_one"] or refused,
                f"uw-01{tag}-undo-silent",
                f"arm ({tag}): `undo` neither restored to turn two's start nor said why it refused — HEAD {r['head_after_undo']}, said {r['undo_said']}",
            )
        cx.rec.notes["outcomes"] = {"a": a["outcome"] if "outcome" in a else ("said: " + str(a["undo_said"])), "b": b["outcome"], "c": c3["outcome"]}
        cx.check()

    # ── #444: `.arbos/` into `.git/info/exclude` ───────────────────────────
    @reg("uw-02-arbos-not-excluded-from-the-users-repo-is-said-once-per-start", tags=("git", "destructive-order"))
    def uw02(cx):
        """The write that keeps the agent's records out of the person's repository, made to fail before any line
        exists: `.git/info` read-only, so `.arbos/` is never excluded and their next `git add -A` stages every
        transcript and checkpoint. Expected of the kernel: it still serves, and root's transcript carries one
        failed notice naming `.gitignore` as the thing to do before committing — once per start, not once a
        turn. The staging itself is recorded on both builds, because the fix is the telling, not the stopping."""
        place = cx.place
        place.mkdir(parents=True, exist_ok=True)
        user_repo(place)
        plain_agent(place)
        info = place / ".git" / "info"
        info.mkdir(parents=True, exist_ok=True)
        exclude = info / "exclude"
        exclude.write_text("# this project's own local ignores\n")
        os.chmod(exclude, 0o444)
        info.chmod(0o555)
        k = cx.kernel(extra_args=["--provider", "replay", "--replies", str(replies_file(cx, [{"agent": "root", "content": "One."}, {"agent": "root", "content": "Two."}]))])
        try:
            started = k.start()
            cx.rec.expect(started, "uw-02-kernel-did-not-start", "the kernel did not come up with .git/info read-only; serving must not depend on that write")
            if not started:
                return
            c = k.attach()
            c.wait(lambda f: f.get("type") == "snapshot", 5)
            c.user("root", "Say one.")
            wait_turn_complete(c, "root", 60)
            c.user("root", "Say two.")
            wait_turn_complete(c, "root", 60)
            evs, _ = transcript(place, "root")
            said = [e for e in notices(evs) if "exclude" in e.get("text", "").lower() or ".gitignore" in e.get("text", "").lower()]
            excluded = "arbos" in (exclude.read_text(errors="replace") if exclude.exists() else "")
            # What is still true after the fix: their `git add -A` would take the agent's records.
            would_stage = [p for p in git_out(place, "status", "--porcelain", "--untracked-files=all").splitlines() if ".arbos" in p][:4]
            frames_said = [f for _, f in c.frames if f.get("type") == "event" and "exclude" in json.dumps(f.get("event", {})).lower()]
            cx.rec.notes.update({
                "exclude_file_holds_arbos": excluded,
                "notices_naming_the_exclude": [e.get("text", "")[:240] for e in said],
                "notice_count": len(said),
                "notice_failed_flag": [bool(e.get("failed")) for e in said],
                "would_be_staged_by_git_add_all": would_stage,
                "event_frames_naming_it": len(frames_said),
                "kernel_still_answering": not c.closed,
            })
            cx.rec.expect(not excluded, "probe-exclude-was-written-anyway", f"the exclude file holds an `.arbos` line although the folder was read-only; the fault was not staged: {exclude.read_text(errors='replace')[:120]!r}")
            cx.rec.expect(
                bool(said),
                "uw-02-silent-about-the-unexcluded-records",
                f"`.arbos/` could not be excluded and nothing was said on root's transcript; the person's next `git add -A` stages {len(would_stage)} agent path(s) ({would_stage}) with no warning anywhere",
                "arbos-core files.rs init_arbos_repo/exclude_locally — say it on root's transcript (#444)",
            )
            if said:
                text = said[-1].get("text", "")
                cx.rec.expect(all(bool(e.get("failed")) for e in said), "uw-02-notice-not-marked-failed", f"the notice is not marked failed, so a window draws it as ordinary prose: {text[:160]!r}")
                cx.rec.expect(".gitignore" in text or "gitignore" in text.lower(), "uw-02-notice-names-no-remedy", f"the notice says what happened but not what to do about it: {text[:200]!r}")
                cx.rec.expect(len(said) <= 1, "uw-02-notice-repeated", f"the notice appeared {len(said)} times over two turns; once per start is the contract and a line a turn teaches people to scroll past")
            cx.rec.expect(bool(would_stage), "probe-nothing-would-be-staged", "no `.arbos` path shows as untracked, so this run does not stand for the harm it names")
        finally:
            info.chmod(0o755)
            os.chmod(exclude, 0o644)
            k.stop()
        cx.check()

    # ── #444: the panic path's unrecorded end ──────────────────────────────
    @reg("uw-03-a-panicked-turn-that-cannot-be-recorded-is-still-said", tags=("panic", "destructive-order"))
    def uw03(cx):
        """`pn-01` drives the panic path with a transcript that works. This drives the half nobody had: the turn
        panics (ARBOS_TEST_PANIC_TURN=root) while root's transcript cannot be appended to, so the words the
        kernel meant to write have nowhere to go. Expected: the fault is still visible — `turn_panicked_unrecorded`
        in the kernel log and the notice delivered to the attached client as a frame — and the turn does not
        read as still running. On the control the fault is invisible in every place a person or a next boot looks."""
        place = cx.place
        place.mkdir(parents=True, exist_ok=True)
        cx.rec.notes["expected_panic"] = "the turn task panicked on purpose"  # the harness's panic detector lets this one through
        plain_agent(place)
        transcript_file = place / ".arbos" / "agents" / "root" / "transcript.jsonl"
        transcript_file.write_text("")
        os.chmod(transcript_file, 0o444)
        env = {**cx.env, "ARBOS_TEST_PANIC_TURN": "root"}
        from run import Kernel  # the harness's own launcher, so ns-wrap and the stderr capture still apply

        k = Kernel(cx.binary, place, cx.rec, env, tag="panicking", extra_args=["--provider", "replay", "--replies", str(replies_file(cx, [{"agent": "root", "content": "Never reached."}]))])
        cx.kernels.append(k)
        try:
            started = k.start()
            cx.rec.expect(started, "uw-03-kernel-did-not-start", "the kernel did not come up with root's transcript unwritable")
            if not started:
                return
            c = k.attach()
            c.wait(lambda f: f.get("type") == "snapshot", 5)
            c.user("root", "This turn will panic before it can write anything.")
            # `idle` on purpose here, and the only case in this module: the turn's end cannot reach the
            # transcript, so no `turn_complete` event can be broadcast to wait on. The wait is bounded and
            # its timing out is data, not a failure.
            idle = c.wait_turn("root", "idle", 45)
            time.sleep(1.0)
            rows = kernel_rows(place)
            panicked = [r for r in rows if "panic" in str(r.get("event", "")).lower()]
            unrecorded = [r for r in rows if r.get("event") == "turn_panicked_unrecorded"]
            # Frames, not the file: the fix's escape route when the transcript will not take the words.
            event_frames = [f for _, f in c.frames if f.get("type") == "event"]
            said_in_a_frame = [f for f in event_frames if "internal error" in json.dumps(f.get("event", {})).lower()]
            turns_dir = place / ".arbos" / "agents" / "root" / "turns"
            metas = sorted(turns_dir.glob("*/meta.toml")) if turns_dir.exists() else []
            marked = [p for p in metas if "error" in p.read_text(errors="replace")]
            # Does the next boot replay the wake and run the turn again? The transcript is readable again first.
            os.chmod(transcript_file, 0o644)
            k.stop()
            k2 = cx.kernel(tag="after-restart", extra_args=["--provider", "replay", "--replies", str(replies_file(cx, [{"agent": "root", "content": "Replayed."}], "replies-restart.jsonl"))])
            replayed = None
            if k2.start():
                c2 = k2.attach()
                c2.wait(lambda f: f.get("type") == "snapshot", 5)
                time.sleep(4.0)
                evs2, _ = transcript(place, "root")
                replayed = [e.get("kind") for e in evs2][-6:]
                k2.stop()
            cx.rec.notes.update({
                "turn_reached_idle": idle is not None,
                "kernel_log_panic_rows": [{"event": r.get("event"), "detail": str(r.get("detail"))[:160]} for r in panicked][:4],
                "turn_panicked_unrecorded_rows": len(unrecorded),
                "notice_delivered_as_a_frame": len(said_in_a_frame),
                "event_frames_seen": len(event_frames),
                "turn_folders": len(metas),
                "turn_folders_carrying_an_error": [p.parent.name for p in marked],
                "transcript_bytes_after": transcript_file.stat().st_size,
                "kinds_after_restart": replayed,
            })
            visible = bool(panicked) or bool(said_in_a_frame) or bool(marked)
            cx.rec.expect(
                visible,
                "uw-03-panic-with-an-unwritable-transcript-is-invisible",
                f"the turn panicked and the transcript could not take the words: kernel log panic rows {len(panicked)}, notice frames {len(said_in_a_frame)}, turn folders carrying the error {len(marked)} — a person sees a turn that simply stopped and the next boot has nothing to tell it the wake was finished",
                "arbos-kernel sched.rs — mark the turn folder, log turn_panicked_unrecorded, broadcast the notice (#444)",
            )
            cx.rec.expect(
                bool(said_in_a_frame) or bool(unrecorded),
                "uw-03-nothing-reached-a-window-or-the-log-by-name",
                f"nothing named the unrecordable end: `turn_panicked_unrecorded` rows {len(unrecorded)}, notice frames {len(said_in_a_frame)}. The window is the only reader left when the file refuses",
                "arbos-kernel sched.rs — the words go to the windows live when the transcript refuses them (#408's rule)",
            )
            cx.rec.expect(transcript_file.stat().st_size == 0, "probe-transcript-took-the-words-anyway", f"the transcript grew to {transcript_file.stat().st_size} bytes although it was read-only; the fault was not staged")
        finally:
            try:
                os.chmod(transcript_file, 0o644)
            except OSError:
                pass
        cx.check()

    # ── #444: a subscription job's marker ──────────────────────────────────
    @reg("uw-04-subscription-job-whose-marker-cannot-be-written-is-ended-not-orphaned", tags=("subscriptions", "jobs", "destructive-order"))
    def uw04(cx):
        """The marker naming whose run a job is, written just after the process starts. Unwritten, a kernel that
        finds the job at boot cannot place it and the run's result reaches nobody. The window is a few
        milliseconds wide, so a watcher takes the write bit off the job folder the instant it appears and the
        run is repeated up to eight times; the rate is reported. Expected on a build that checks the write: the
        command is ended and the run said to have failed to start. Never catching the window is a skip."""
        place = cx.place
        place.mkdir(parents=True, exist_ok=True)
        plain_agent(place)
        # The harness's own writer, not a hand-rolled TOML: a file without `id`/`created`/`next_due` is
        # dropped without a word (qa-029), and a probe staged that way measures that bug instead of this one.
        from fileplan_scenarios import write_subscription

        marker_dir = place / ".arbos" / "agents" / "root" / "jobs"
        marker_dir.mkdir(parents=True, exist_ok=True)
        # 30s is the kernel's minimum and it says so on the log when a file asks for less; each firing is
        # one attempt at the window below.
        write_subscription(place, "root", "uw04", kind="shell", cmd="sleep 20 && echo uw04-ran-to-the-end", every="30s", deliver_to="user", notify="uw04: {output}")
        caught, attempts, stopped = 0, 0, []
        hit = threading.Event()
        watching = threading.Event()
        watching.set()

        def watcher():
            """Put a directory at the marker's own path the instant a job folder appears, so that one write
            fails and nothing else in the folder does. The world's cause is the disk filling in the few
            milliseconds between the process starting and its marker being written; a directory is not that
            cause, but it is the only way to fail this single write from outside, and the question under test
            — what happens to a run whose marker never landed — does not depend on the errno. Everything else
            in the job folder (out.log, exit, meta) still works, which a read-only folder would not allow."""
            nonlocal caught
            seen = set()
            while watching.is_set():
                try:
                    for d in os.scandir(marker_dir):
                        if d.is_dir() and d.name not in seen:
                            seen.add(d.name)
                            try:
                                (Path(d.path) / "subscription").mkdir()
                                caught += 1
                                hit.set()
                            except OSError:
                                pass  # the marker was already there: this firing was not caught
                except OSError:
                    pass
                time.sleep(0.0002)

        k = cx.kernel(extra_args=["--provider", "replay", "--replies", str(replies_file(cx, [{"agent": "root", "content": "Noted."}] * 6))])
        t = threading.Thread(target=watcher, daemon=True)
        try:
            started = k.start()
            cx.rec.expect(started, "uw-04-kernel-did-not-start", "the kernel did not come up")
            if not started:
                return
            t.start()
            c = k.attach()
            c.wait(lambda f: f.get("type") == "snapshot", 5)
            # every = 30s, so one attempt at the window every half minute; three minutes gives six.
            deadline = time.time() + 185
            while time.time() < deadline and caught < 3:
                time.sleep(0.5)
            watching.clear()
            time.sleep(1.0)
            attempts = len([d for d in marker_dir.iterdir() if d.is_dir()])
            # Is anything still running that nobody can place? The wrapper's own process is the evidence.
            # pid and the first words only: the leash's own script is thousands of characters and a break
            # message nobody can read is a break nobody acts on.
            leftover = [l[:90] for l in subprocess.run(["pgrep", "-af", "uw04-ran-to-the-end"], capture_output=True, text=True).stdout.strip().splitlines()]
            unplaceable = []
            for d in sorted(marker_dir.iterdir()):
                if not d.is_dir():
                    continue
                try:
                    d.chmod(0o755)
                except OSError:
                    pass
                has_marker = (d / "subscription").is_file()  # a directory there is this probe's own injection
                exited = (d / "exit").exists()
                out = (d / "out.log").read_text(errors="replace")[:120] if (d / "out.log").exists() else ""
                if not has_marker:
                    unplaceable.append({"job": d.name, "exit_file": exited, "out": out})
            evs, _ = transcript(place, "root")
            # Any event, not only a notice: the kernel delivers a failed subscription run as the next turn's
            # `wake` ("Subscription #1 (...) failed: exit -1. Output tail: could not start ..."), which is what
            # the person actually reads. A probe that looked only at notices called a working path silent.
            heard = [
                f"{e.get('kind')}: {str(e.get('text', ''))[:160]}"
                for e in evs
                if "record could not be written" in str(e.get("text", "")) or "could not start" in str(e.get("text", ""))
            ]
            notice_text = heard
            rows = kernel_rows(place)
            cx.rec.notes.update({
                "job_folders": attempts,
                "folders_caught_before_the_marker": caught,
                "folders_left_without_a_marker": unplaceable[:6],
                "processes_still_running_after": leftover[:4],
                "notices_naming_the_record": notice_text[:4],
                "kernel_log_events": sorted({str(r.get("event")) for r in rows})[:20],
            })
            if caught == 0:
                cx.rec.notes["skipped"] = "self: probe-never-caught-the-window — no job folder was seen before its marker was written; this run proves nothing about the marker's failure"
                return
            # The claim: the command is ended, and the run is reported as not started. Either the folder
            # carries no orphan process, or something said the record could not be written.
            # The destructive claim: a run whose marker never landed must not go on to finish. `exit_file`
            # with output and no marker is a finished run nobody can attribute — the "finished and
            # unnoticed" class the audit names.
            finished_unattributable = [u for u in unplaceable if u["exit_file"] or u["out"].strip()]
            cx.rec.notes["finished_without_a_marker"] = finished_unattributable
            cx.rec.notes["heard_by_the_person"] = heard[:3]
            cx.rec.expect(
                not finished_unattributable,
                "uw-04-run-finished-with-no-record-of-whose-it-is",
                f"{len(finished_unattributable)} of {caught} caught job(s) ran to the end with no marker ({[u['job'] for u in finished_unattributable]}): their output is on disk and a restart cannot say whose run it was, so the result reaches nobody",
                "arbos-kernel subs.rs run_job — an unwritten marker ends the run before it goes on unrecorded (#444)",
            )
            cx.rec.expect(
                bool(heard),
                "uw-04-nobody-was-told-the-run-did-not-start",
                f"{caught} job folder(s) could not take their marker; {len(leftover)} process(es) still run with nothing to place them (pids {[l.split()[0] for l in leftover if l.split()][:4]}), {len([u for u in unplaceable if u['exit_file']])} of them already finished with output nobody can attribute; nothing was said on the transcript. A restart now finds a job it cannot place and the run's result reaches nobody",
                "arbos-kernel subs.rs run_job — an unwritten marker ends the run and says so (#444)",
            )
        finally:
            watching.clear()
            for d in list(marker_dir.iterdir()) if marker_dir.exists() else []:
                try:
                    d.chmod(0o755)
                    marker = d / "subscription"
                    if marker.is_dir():
                        marker.rmdir()
                except OSError:
                    pass
            k.stop()
            subprocess.run(["pkill", "-f", "uw04-ran-to-the-end"], capture_output=True)
        cx.check()

    # ── the symmetry loop's inbox note of 20:30: a moved place comes back ──
    @reg("af-04-a-moved-places-old-path-is-not-recreated-by-the-kernels-late-writes", needs_model=True, tags=("after-failure", "destructive-order"))
    def af04(cx):
        """From `internal/features-inbox/2026-09-17-kernel-writes-recreate-a-moved-place.md`: the gate renamed
        a place under an idle kernel and found the old path **back**, holding the whole tree but no
        `agents/<id>`, still growing — the kickoff's tail (`notes.md`, `spend.toml`,
        `notifications.jsonl`, `kernel.log`) landing seconds late through `create_dir_all` on the absolute
        path. The process's cwd followed the inode; its writes did not. af-03's rule checks at turn start,
        so an idle kernel finishing a tail is outside it, and the next person to open that path reads a
        ghost project.

        The rename is timed off the turn's `turn_complete` **event**, not `idle`: `idle` arrives after the
        notes nudge, which is itself part of the tail this is trying to race. The note saw it in 1 run of
        3; this waits on the earlier frame to make the window as wide as the kernel allows, and reports
        what it caught either way."""
        place = cx.place
        place.mkdir(parents=True, exist_ok=True)
        moved = place.parent / (place.name + "-moved")
        # A real model: the tail this races is the kickoff's, and the kickoff does not run under the
        # replay provider — the probe's second version waited 90 s for a `turn_complete` that never came
        # and then renamed the folder long after anything was being written (`turn_complete_seen: false`).
        k = cx.kernel()
        try:
            started = k.start()
            cx.rec.expect(started, "af-04-kernel-did-not-start", "the kernel did not come up")
            if not started:
                return
            c = k.attach()
            c.wait(lambda f: f.get("type") == "snapshot", 5)
            # The kickoff is client-initiated: the desktop sends a `kickoff` frame, and a kernel merely
            # served never runs one (measured: the kernel log of the second version held `kernel_start`,
            # `attach_open`, `notify_replayed`, `kernel_stop` and no turn at all). The tail this races is
            # the kickoff's own — the plan write to notes.md, the spend line, the greeting's notification
            # — so the frame is sent here rather than a user line, whose tail is a different, smaller one.
            c.send({"type": "kickoff", "agent": "root"})
            ended = wait_turn_complete(c, "root", 120)
            # The rename goes in the moment the turn's end is recorded, while its tail is still landing.
            renamed_at = time.time()
            place.rename(moved)
            tail = sorted(p.name for p in moved.rglob("*") if p.is_file())[:0]  # touch nothing, just prove the move
            cx.rec.notes["turn_complete_seen"] = ended is not None
            cx.rec.notes["moved_to"] = str(moved)
            if ended is None:
                # No kickoff turn ended, so there is no tail to race and this run establishes nothing
                # about the property. Said, not scored: a probe that stages nothing must not read as a
                # pass (the audit of 2026-09-17, twenty-nine scenarios).
                cx.rec.notes["skipped"] = "self: probe-no-kickoff-turn — no turn_complete in 90 s, so the rename did not land inside the kickoff's tail and nothing was staged"
                return
            # Poll the old path: the fact asserted is "nothing is recreated here", so it is watched for a
            # bounded time rather than sampled once (the note's writes arrived up to 70 s after the move).
            recreated, grew = None, []
            deadline = time.time() + 75
            while time.time() < deadline:
                if place.exists():
                    files = sorted(str(p.relative_to(place)) for p in place.rglob("*") if p.is_file())
                    if files or any(place.iterdir()):
                        recreated = {"after_s": round(time.time() - renamed_at, 1), "files": files[:12], "entries": sorted(p.name for p in place.iterdir())[:12]}
                        grew = files
                        break
                time.sleep(0.5)
            if recreated:
                time.sleep(8)
                after = sorted(str(p.relative_to(place)) for p in place.rglob("*") if p.is_file()) if place.exists() else []
                recreated["still_growing"] = len(after) > len(grew)
                recreated["files_after_8s"] = len(after)
                recreated["has_agents_dir"] = (place / ".arbos" / "agents").exists()
                recreated["has_root_agent"] = (place / ".arbos" / "agents" / "root").exists()
            cx.rec.notes["old_path_recreated"] = recreated
            cx.rec.notes["kernel_alive_after"] = k.alive()
            # Every run of this scenario also leaves `runtime/lock` behind, holding the stopped kernel's
            # pid. `lock.rs` says that is not cosmetic — "a lock file left behind reads to the next
            # kernel and to `check` as a holder that is not there" — and it has `release_at(place_now)`
            # for exactly this case, a folder that moved under the kernel. So ask the outcome instead of
            # classifying it: can a new kernel serve the place where the folder now is?
            served = None
            if moved.exists():
                k2 = cx.kernel(tag="after-move", place=moved)
                served = k2.start()
                lock2 = moved / ".arbos" / "runtime" / "lock"
                cx.rec.notes["after_move"] = {
                    "new_kernel_served_the_moved_folder": served,
                    "leftover_lock_pid": (lock2.read_text(errors="replace").strip()[:12] if lock2.exists() else None),
                    "old_kernel_pid": k.proc.pid if k.proc else None,
                }
                k2.stop()
                cx.rec.expect(
                    served,
                    "af-04-moved-folder-cannot-be-served-again",
                    f"after the folder moved and its kernel stopped, a new kernel could not serve it at {moved.name}: the lock file left behind names pid {cx.rec.notes['after_move']['leftover_lock_pid']}, which is gone. `lock.rs::release_at` exists for this",
                    "arbos-core lock.rs Drop/release_at — release by the folder's current path when it moved",
                )
            cx.rec.expect(
                recreated is None,
                "af-04-old-path-recreated-as-a-ghost-project",
                f"the place was renamed to {moved.name} and its old path came back {recreated['after_s'] if recreated else '?'}s later with {recreated['entries'] if recreated else ''} — agents/ {'present' if recreated and recreated['has_agents_dir'] else 'absent'}, root agent {'present' if recreated and recreated['has_root_agent'] else 'absent'}, still growing: {recreated.get('still_growing') if recreated else '?'}. The next open of that path reads a project that is a shell",
                "arbos-kernel/core: check the place is the folder opened at start before any write under it, or write through a handle opened then (the symmetry loop's ask, 2026-09-17 20:30)",
            )
        finally:
            if moved.exists() and not place.exists():
                try:
                    moved.rename(place)
                except OSError:
                    pass
            k.stop()
        cx.check(place=place if place.exists() else moved)

    # ── #450: the detector for a double-serving that has already happened ──
    @reg("ds-01-the-double-serving-detector-finds-both-shapes-and-stays-quiet-on-the-innocent-ones", tags=("detector", "lock", "after-failure"))
    def ds01(cx):
        """#450 (`a9b12f118650`) says two kernels on one place leave two marks: a turn's wake written while
        another turn of the same agent was still open with no cut line between, and two checkpoints at one
        transcript line with different times. Its worth is not the warning but the reading — the question
        it answers is whether a person's places were served twice while the lock was split, so its
        precision matters as much as its recall. A detector that also fires on an interrupted kernel or an
        ordinary place sends everyone hunting for a fault they never had.

        Four places, built by hand and read through `arbos-kernel check` as a person reads it: the two
        shapes it must find, and two innocent states it must stay quiet on — a kernel that died mid-turn
        with the restart notice that documents the clearing, and a plain place with two clean turns.

        Measured 2026-09-18: on `arbos-kernel 0.2.0 d373422662bd protocol 1` all four arms behave, and on
        `f80f0b663bac`, which predates #450, all four are silent — so the warnings come from the detector
        and not from the staging."""
        root = cx.scratch / "ds01"

        def place(name, transcript_lines, checkpoint_lines=()):
            p = root / name
            (p / ".arbos" / "agents" / "root").mkdir(parents=True, exist_ok=True)
            (p / ".arbos" / "agents" / "root" / "agent.md").write_text("root\n")
            (p / ".arbos" / "agents" / "root" / "transcript.jsonl").write_text("".join(l + "\n" for l in transcript_lines))
            if checkpoint_lines:
                (p / ".arbos" / "agents" / "root" / "checkpoints.jsonl").write_text("".join(l + "\n" for l in checkpoint_lines))
            return p

        wake = lambda ts: json.dumps({"ts": ts, "kind": "wake", "wake": "user", "text": "go"})
        done = lambda ts: json.dumps({"ts": ts, "kind": "turn_complete"})
        note = lambda ts, t: json.dumps({"ts": ts, "kind": "notice", "text": t})
        cp = lambda line, ts, head: json.dumps({"line": line, "ts": ts, "head": head, "clean": True})

        def warnings_of(p):
            """The lines a person reads. PROTOCOL.md's absence is an artefact of a hand-built place and is
            not one of them, so only the double-serving wordings count."""
            out = subprocess.run([cx.binary, "check", str(p)], capture_output=True, text=True, timeout=60)
            text = (out.stdout or "") + (out.stderr or "")
            return [l.strip() for l in text.splitlines() if any(k in l for k in ("still open", "two kernels", "two checkpoints"))]

        arms = {
            "a_wake_while_the_previous_turn_is_open": (place("a", [wake(1000), done(1100), wake(2000), wake(3000)]), True),
            "b_two_checkpoints_one_line_different_times": (place("b", [wake(1000), done(1100)], [cp(0, 1000, "aaa"), cp(0, 2000, "bbb")]), True),
            "c_died_mid_turn_with_the_restart_notice": (place("c", [wake(1000), note(1500, "kernel restarted"), wake(2000), done(2100)]), False),
            "d_an_ordinary_place_two_clean_turns": (place("d", [wake(1000), done(1100), wake(2000), done(2100)], [cp(0, 1000, "aaa"), cp(2, 2000, "bbb")]), False),
        }
        seen = {}
        for name, (p, want) in arms.items():
            ws = warnings_of(p)
            seen[name] = {"expected": "warn" if want else "quiet", "warnings": [w[:200] for w in ws]}
        cx.rec.notes["arms"] = seen

        for name, (p, want) in arms.items():
            ws = seen[name]["warnings"]
            said = name.split("_", 1)[1].replace("_", " ")
            if want:
                cx.rec.expect(
                    bool(ws),
                    f"ds-01-missed-{name}",
                    f"`check` says nothing about {said}, so a place served by two kernels reads as sound. This is the shape #450 added the detector for",
                    "arbos-kernel check.rs check_two_writers",
                )
            else:
                cx.rec.expect(
                    not ws,
                    f"ds-01-false-alarm-on-{name}",
                    f"`check` reports double serving for {said}, which is an ordinary state: {ws[:1]}. A warning that fires on a healthy place cannot be used to decide whether anything was served twice",
                    "arbos-kernel check.rs check_two_writers — the cut line must clear the open wake",
                )

    # ── a write that runs out of room: the supported path into drop_partial_line ──
    @reg("en-01-a-write-that-runs-out-of-room-does-not-swallow-the-next-event", tags=("after-failure", "transcript", "destructive-order"))
    def en01(cx):
        """The complement to `qal-j37`. `drop_partial_line` exists for a write that fails part-way and
        names its causes: *"disk full, size limit"*. `qal-j37` showed the **crash** path has no repair
        at all — nobody runs the guard when the process is gone. This asks whether the guard works on
        the path it was written for, by the cheapest of its two causes: `RLIMIT_FSIZE`, set on the
        kernel before exec, with the transcript padded to just under it so the next append crosses it
        mid-write.

        A size limit is reachable without anything exotic: a quota, a container limit, a filesystem's
        own ceiling. Whether the process even *gets* an error is the first question — exceeding
        `RLIMIT_FSIZE` raises `SIGXFSZ`, whose default action is to kill, so a guard on the `write_all`
        error path only runs if that signal is handled or ignored. If the kernel dies instead, then
        the comment's "size limit" reaches `qal-j37`'s territory rather than the guard's, which is
        worth knowing either way.

        Whatever happens, the property is the same as `qal-j37`'s: no event may end up inside an
        unparseable line, and nothing already whole may be lost."""
        place = cx.place
        place.mkdir(parents=True, exist_ok=True)
        plain_agent(place)
        k0 = cx.kernel(tag="kernel-bootstrap")
        cx.rec.expect(k0.start(), "en-01-kernel-did-not-bootstrap", "the kernel did not come up to make its folders")
        k0.stop()

        tpath = place / ".arbos" / "agents" / "root" / "transcript.jsonl"
        tpath.parent.mkdir(parents=True, exist_ok=True)
        # RLIMIT_FSIZE is in bytes and applies to every file this process writes, so the ceiling has
        # to clear the kernel's own small files (kernel.log, kernel.json) and sit just above the
        # transcript. Pad with whole, readable events so nothing is lost by the padding itself.
        limit = 64 * 1024
        pad = {"ts": now_ms() - 1, "kind": "assistant", "text": "x" * 400}
        line = json.dumps(pad) + "\n"
        with open(tpath, "w") as f:
            f.write(json.dumps({"ts": now_ms() - 2, "kind": "wake", "wake": "user", "text": "one"}) + "\n")
            while tpath.stat().st_size < limit - 600:
                f.write(line)
                f.flush()
            f.write(json.dumps({"ts": now_ms() - 1, "kind": "turn_complete"}) + "\n")

        def read_lines():
            good, bad = [], []
            for raw in tpath.read_text(errors="replace").split("\n"):
                if not raw.strip():
                    continue
                try:
                    good.append(json.loads(raw))
                except ValueError:
                    bad.append(raw)
            return good, bad

        good0, bad0 = read_lines()

        def cap_file_size():
            resource.setrlimit(resource.RLIMIT_FSIZE, (limit, limit))

        k = cx.kernel(tag="kernel-capped", preexec=cap_file_size,
                      extra_args=["--provider", "replay", "--replies", str(replies_file(cx, [{"agent": "root", "content": "ROOM"}]))])
        try:
            started = k.start()
            cx.rec.notes["kernel_started_under_the_cap"] = started
            if not started:
                cx.rec.notes["skipped"] = "the kernel could not start with RLIMIT_FSIZE at 64 KiB; the cap is below its own bootstrap writes, so this run stages nothing"
                return
            c = k.attach()
            c.wait(lambda f: f.get("type") == "snapshot", 5)
            marker = f"ROOM-{now_ms() % 100000}"
            c.user("root", f"{marker}: reply with the single word ROOM.")
            wait_turn_complete(c, "root", 60)
            time.sleep(1)

            good1, bad1 = read_lines()
            swallowed = [b for b in bad1 if marker in b]
            err = k.stderr_text() or ""
            cx.rec.notes.update({
                "readable_before": len(good0),
                "readable_after": len(good1),
                "unparseable_before": len(bad0),
                "unparseable_after": len(bad1),
                "bytes_before": limit - 600,
                "bytes_after": tpath.stat().st_size,
                "kernel_alive_after": k.alive(),
                "stderr_mentions_size_or_space": any(w in err.lower() for w in ("file too large", "efbig", "no space", "enospc", "size limit")),
                "unparseable_tail": [b[:160] for b in bad1][:2],
            })

            # Probe validity: the transcript must actually have met the ceiling. If the append fitted,
            # nothing was staged and the arms below say nothing.
            met_the_ceiling = tpath.stat().st_size >= limit - 600 and (len(bad1) > len(bad0) or not k.alive() or cx.rec.notes["stderr_mentions_size_or_space"])
            cx.rec.expect(
                met_the_ceiling,
                "probe-the-write-had-room",
                f"the transcript grew to {tpath.stat().st_size} under a {limit}-byte cap with no failure sign, so the append had room and this run does not stage a write that ran out of it",
            )
            if not met_the_ceiling:
                return

            cx.rec.expect(
                len(good1) >= len(good0),
                "en-01-earlier-events-lost",
                f"the transcript held {len(good0)} readable events before and {len(good1)} after; a write that ran out of room must cost nothing that was already whole",
            )
            cx.rec.expect(
                not swallowed,
                "en-01-event-swallowed-by-the-headless-line",
                f"an event is inside an unparseable line after a write ran out of room: {(swallowed[0][:150] if swallowed else '')!r}. `drop_partial_line` is meant to cut the headless remainder off on exactly this path",
                "arbos-core files.rs append_events / drop_partial_line — the failed-write path",
            )
        finally:
            k.stop()

    # ── diagnostic: does a sub-chat's turn start under the harness at all? ──
    @reg("dg-01-a-sub-chats-turn-starts-under-the-harness", needs_model=True, tags=("desktop", "diagnostic"))
    def dg01(cx):
        """`mt-01` and `mt-04` are the only two scenarios that wait on a session reporting `streaming`
        or `turn_open`, and both time out at 40 s on the app at `1beec0a1fd98` while everything they
        do works when driven by hand outside the harness (`qal-j42`). Helper, payload, predicate and
        the fields themselves are all eliminated. The one difference left is the launch: the harness
        starts the app through ns-wrap and my probe did not.

        So: the same steps, inside the harness, recording a timeline instead of a verdict — the flags
        each second and the chat agent's transcript growing — so "the turn never starts" and "the flag
        never shows" can be told apart."""
        if not desktop_scenarios.available():
            cx.rec.notes["skipped"] = "desktop binary/driver/Xvfb missing"
            return
        d = desktop_scenarios.Desktop(cx)
        try:
            sid = d.new_chat()
            agent = d.agent
            cx.rec.notes["agent"] = agent
            d.send("Reply with the single word DGOK and stop.")
            tpath = cx.place / ".arbos" / "agents" / str(agent) / "transcript.jsonl"
            timeline = []
            for i in range(40):
                st = d.app.state()
                flags = [
                    (c.get("agent_session"), bool(c.get("streaming")), bool(c.get("turn_open")), c.get("connection"))
                    for p in st["projects"] for c in p.get("sessions") or []
                ]
                lines = len(tpath.read_text(errors="replace").splitlines()) if tpath.exists() else 0
                timeline.append({"t": i, "lines": lines, "flags": flags})
                # Only this chat's own session counts. `any(...)` over every session is
                # satisfied by **root's kickoff turn**, which is running at exactly this moment in
                # a fresh place — the red herring qal-j43 is built on. Cycle 16 showed the cost:
                # the loop exited after 1 s because root was streaming, so `seconds_watched` was 1
                # and the second assertion passed while the chat had nothing on its transcript.
                if any(f[0] == agent and (f[1] or f[2]) for f in flags):
                    break
                time.sleep(1)
            ever = any(any(f[0] == agent and (f[1] or f[2]) for f in e["flags"]) for e in timeline)
            cx.rec.notes.update({
                "streaming_or_open_ever": ever,
                "transcript_lines_final": timeline[-1]["lines"],
                "seconds_watched": len(timeline),
                "first": timeline[0],
                "last": timeline[-1],
            })
            cx.rec.expect(
                timeline[-1]["lines"] > 0,
                "dg-01-the-line-never-reached-the-chat",
                f"nothing is on {agent}'s transcript after {len(timeline)} s, so the send did not arrive under the harness: {timeline[-1]}",
            )
            cx.rec.expect(
                ever,
                "dg-01-no-session-ever-reported-a-turn",
                f"{agent}'s transcript has {timeline[-1]['lines']} line(s) but no session reported streaming or turn_open in {len(timeline)} s: {timeline[-1]['flags']}. If the transcript grew, the turn ran and the flag is the problem; if it did not, the turn never started",
            )
        finally:
            d.close()

    # ── the kickoff race itself, minted on purpose (qal-j43) ──
    @reg("kf-01-a-chat-opened-during-kickoff-keeps-what-you-type", needs_model=True, tags=("desktop", "after-failure"))
    def kf01(cx):
        """`qal-j43`: on app builds before `1768ec83`, a chat minted while its place was still
        serving the **kickoff turn** was inert. The app minted it, activated it, reported its
        connection live, and the composer cleared on Enter — but the typed line never reached
        `Session::send()` at all, so no frame went on the wire and the kernel never heard it.

        Fixed on `main` by `1768ec83` ("desktop: chat fills the column; Clear goes; expand is
        pinned at the window's top-right", in [#656](https://github.com/unarbos/arbos/pull/656)),
        which is a **layout** commit. Nothing in it names this bug, so the fix was almost certainly
        incidental — which is the whole reason this scenario exists. Bisected 2026-09-18 with an
        instrumented build: `e10953fb` (its parent) breaks 3/3, `1768ec83` passes 3/3.

        The contract: a cleared composer means the app accepted the line, and an accepted line must
        arrive. Minting deliberately inside the kickoff window is the only way to hold it."""
        if not desktop_scenarios.available():
            cx.rec.notes["skipped"] = "desktop binary/driver/Xvfb missing"
            return
        line = "Reply with the single word KFOK and stop."

        def attempt(n):
            """One go at the window. True when it settled the question either way.

            The window is a few seconds wide and a slow launch can miss it, leaving a run that
            proves nothing — and, because nothing broke, reports as a **pass**. Two of the first
            fifty-one runs did exactly that. For a fault that is a rate rather than a switch
            (5 of 6 at `2ea8d565`, 0 of 6 at its parent), a hollow pass is worse than no run, so a
            missed window is retried on a place made new again instead of recorded as a green.
            """
            d = desktop_scenarios.Desktop(cx)
            try:
                root = cx.place / ".arbos" / "agents" / "root" / "transcript.jsonl"

                def kickoff_state():
                    kinds = []
                    for raw in root.read_text(errors="replace").splitlines() if root.exists() else []:
                        try:
                            kinds.append(json.loads(raw).get("kind"))
                        except ValueError:
                            pass
                    return kinds

                # Wait only until the kickoff turn has *started*, then mint inside it. Waiting for
                # it to finish is exactly what this scenario must not do.
                until = time.time() + 40
                while time.time() < until and not kickoff_state():
                    time.sleep(0.2)
                if not kickoff_state() or "turn_complete" in kickoff_state():
                    cx.rec.notes[f"attempt_{n}_missed_the_window"] = str(kickoff_state())
                    return False
                cx.rec.notes["kickoff_running_when_minted"] = True

                before = {c["id"] for c in d.sessions()}
                d.app.key("cmd-n")
                st = d.app.wait_state(
                    lambda s: {c["id"] for p in s["projects"] for c in p["sessions"]} - before,
                    timeout=20, what="a chat minted during kickoff",
                )
                sid = ({c["id"] for p in st["projects"] for c in p["sessions"]} - before).pop()
                agent = next((c.get("agent_session") for p in st["projects"] for c in p["sessions"] if c["id"] == sid), None)
                cx.rec.notes["agent"] = agent

                desktop_scenarios.focus_composer(d.app)
                d.app.type(line)
                d.app.key("enter")
                held = str((d.app.state().get("composer") or {}).get("text") or "")
                accepted = line[:18] not in held
                cx.rec.notes["composer_cleared_so_line_accepted"] = accepted

                tpath = cx.place / ".arbos" / "agents" / str(agent) / "transcript.jsonl"
                landed, until = False, time.time() + 45
                while time.time() < until and not landed:
                    for raw in tpath.read_text(errors="replace").splitlines() if tpath.exists() else []:
                        try:
                            e = json.loads(raw)
                        except ValueError:
                            continue
                        if e.get("kind") == "user" and line[:18] in (e.get("text") or ""):
                            landed = True
                            break
                    time.sleep(0.5)
                cx.rec.notes.update({"transcript_exists": tpath.exists(), "line_landed": landed})

                cx.rec.expect(
                    landed or not accepted,
                    "kf-01-accepted-line-never-arrived",
                    f"the composer cleared, so the app accepted the line, but {agent}'s transcript "
                    f"{'does not exist' if not tpath.exists() else 'never recorded it'} after 45 s "
                    f"(qal-j43). A cleared composer must mean a delivered line.",
                )
                return True
            finally:
                d.close()

        # A second go needs a place that is new again: the kickoff turn happens once per place, so
        # reopening this one would have no window at all. Wiping the store after the app is down
        # restores the only state that matters here — a place nobody has opened.
        for n in (1, 2):
            if attempt(n):
                break
            if n == 1:
                shutil.rmtree(cx.place / ".arbos", ignore_errors=True)
                time.sleep(1)
        else:
            cx.rec.notes["inconclusive"] = "the kickoff window was missed on both attempts; this run says nothing about qal-j43"


    # ── a scheduled command that fails: the other property #104 orphaned ──
    @reg("sf-01-a-shell-subscription-whose-command-fails-tells-somebody", tags=("after-failure", "subscriptions"))
    def sf01(cx):
        """The second scenario `#104` retired. `plan-shell-verdicts` (run.py:900) read a kernel-run
        shell node's exit status four ways — quiet success, loud success, exit 1, a missing command —
        and then held the property that matters after one fails: `shell-failure-silent`, *"no kernel
        wake node after failed shell nodes; the agent is never told"*.

        It defers to `fp-shell-subscription`, whose commands all succeed: every assertion there is
        cadence, no-model-turn, the reading delivered, a bare file honoured. Nothing runs a command
        that fails, so nothing checks that anyone finds out — rule 13 applied to the same skip note
        that produced `qal-j38`.

        A scheduled command that stops working and says nothing is the ordinary shape of this: a
        backup, a sync, a repository chore. The command here proves it ran and then fails."""
        place = cx.place
        place.mkdir(parents=True, exist_ok=True)
        plain_agent(place)
        k0 = cx.kernel(tag="kernel-bootstrap")
        cx.rec.expect(k0.start(), "sf-01-kernel-did-not-bootstrap", "the kernel did not come up to make its folders")
        k0.stop()

        fileplan_scenarios.write_subscription(
            place, "root", "failing", kind="shell",
            cmd="echo ran >> ran.txt; echo boom >&2; exit 1",
            every="30s", deliver_to="user", notify="chore: {output}",
        )

        k = cx.kernel(tag="kernel-failing-sub")
        try:
            started = k.start()
            cx.rec.expect(started, "sf-01-kernel-did-not-start", "the kernel did not come up on a subscription whose command fails")
            if not started:
                return
            c = k.attach()
            c.wait(lambda f: f.get("type") == "snapshot", 5)
            time.sleep(34)

            runs = len((place / "ran.txt").read_text().splitlines()) if (place / "ran.txt").exists() else 0
            evs, _ = transcript(place, "root")
            blob = json.dumps([e for e in evs]).lower()
            frames = json.dumps([f for _, f in c.frames]).lower()
            # The match has to be the *subscription's* failure and nothing else. A first pass looked
            # for "fail" and matched an unrelated "No API key for OpenRouter" notice, which is how a
            # probe passes for a reason that has nothing to do with its claim. `boom` is this
            # command's own stderr and `chore:` is its own notify line; neither can come from
            # anywhere else in this place.
            mine = ("boom", "chore:")
            told_on_transcript = [
                str(e.get("text") or "")[:200] for e in evs
                if e.get("kind") in ("notice", "say") and any(w in str(e.get("text") or "").lower() for w in mine)
            ]
            told_in_frames = any(w in frames for w in mine)
            # Say which side carried it, so a pass can be read. Measured 2026-09-18 on
            # `a8678ac16636`: the command's own stderr reaches the attached client in frames (14
            # lines carrying `boom`), which is the property holding. Keeping the two apart matters
            # because a first pass matched the word "fail" in an unrelated "No API key" notice.
            cx.rec.notes.update({
                "command_runs": runs,
                "told_by": [s for s, ok in (("transcript", bool(told_on_transcript)), ("frames", told_in_frames)) if ok] or ["nobody"],
                "told_on_transcript": told_on_transcript,
                "told_in_a_frame": told_in_frames,
            })

            # Probe validity: the command has to have run and failed, or there is nothing to report.
            cx.rec.expect(
                runs > 0,
                "probe-subscription-never-ran",
                f"the failing command never ran in 34 s, so this run says nothing about how a failure is reported: {cx.rec.notes}",
            )
            if not runs:
                return
            cx.rec.expect(
                bool(told_on_transcript) or told_in_frames,
                "sf-01-a-failing-scheduled-command-is-silent",
                f"the subscription's command ran {runs} time(s) and exited 1 with `boom` on stderr, and nothing reached the person: no notice or say line on root's transcript naming it, no frame carrying it. A chore that has stopped working looks exactly like one that is working",
                "arbos-kernel subscriptions: a non-zero exit must reach the user the way the plan engine's failed nodes woke the agent (plan-shell-verdicts' shell-failure-silent)",
            )
        finally:
            k.stop()
        cx.check()

    # ── a clock that jumped: the two properties that lost their engine ──
    @reg("ck-01-a-subscription-survives-a-clock-jump-without-a-storm-or-being-stranded", tags=("after-failure", "subscriptions", "environment"))
    def ck01(cx):
        """`clock-jump-cron` (run.py:866) held two properties over the old `plan.jsonl` engine:

          - **coalesce** — a node ten days overdue fires *once*, not once per missed period;
          - **rewind recovery** — a node whose `next_due` sits ten days ahead because the clock was
            set back is pulled to within one period, rather than never firing again.

        #104 replaced that engine with `subscriptions/`, so the scenario now sets itself aside with a
        note pointing at `fp-shell-subscription` and `fp-timer-subscription`. Neither carries either
        property: both test the cadence of a subscription that is due now, and `write_subscription`
        defaults `next_due` to one second ago, so nothing in the library has ever put a subscription
        far out of date in either direction. The skip is honest and names successors that do not
        inherit what it was for.

        Both cases are ordinary. A laptop shut for ten days wakes with a 30-second subscription
        28,800 periods behind; a clock corrected backwards (NTP, a timezone fix, a person) leaves one
        due in the future. Shell subscriptions here, so the arms cost no model and the count is exact.
        """
        place = cx.place
        place.mkdir(parents=True, exist_ok=True)
        plain_agent(place)
        k0 = cx.kernel(tag="kernel-bootstrap")
        cx.rec.expect(k0.start(), "ck-01-kernel-did-not-bootstrap", "the kernel did not come up to make its folders")
        k0.stop()

        ten_days = datetime.timedelta(days=10)
        now = datetime.datetime.now(datetime.timezone.utc)
        fileplan_scenarios.write_subscription(
            place, "root", "overdue", kind="shell",
            cmd="echo overdue >> overdue.txt", every="30s", deliver_to="user", notify="overdue",
            next_due=fileplan_scenarios.rfc3339(now - ten_days),
        )
        fileplan_scenarios.write_subscription(
            place, "root", "future", kind="shell",
            cmd="echo future >> future.txt", every="30s", deliver_to="user", notify="future",
            next_due=fileplan_scenarios.rfc3339(now + ten_days),
        )
        subs = sorted((place / ".arbos" / "agents" / "root" / "subscriptions").glob("*.toml"))

        k = cx.kernel(tag="kernel-after-jump")
        try:
            started = k.start()
            cx.rec.expect(started, "ck-01-kernel-did-not-start", "the kernel did not come up on subscriptions whose next_due is ten days out")
            if not started:
                return
            time.sleep(28)

            count = lambda name: len((place / name).read_text().splitlines()) if (place / name).exists() else 0
            overdue_runs, future_runs = count("overdue.txt"), count("future.txt")

            def due_after(path):
                for line in path.read_text(errors="replace").splitlines():
                    if line.strip().startswith("next_due"):
                        return line.split("=", 1)[1].strip().strip('"')
                return None

            dues = {p.name: due_after(p) for p in subs}
            cx.rec.notes.update({
                "overdue_runs_in_28s": overdue_runs,
                "future_runs_in_28s": future_runs,
                "next_due_on_disk": dues,
                "periods_missed": 10 * 24 * 60 * 2,
            })

            # Probe validity: the kernel must be running subscriptions at all, or neither arm means
            # anything. The overdue one being due is the cheapest proof of that.
            cx.rec.expect(
                overdue_runs > 0,
                "probe-subscriptions-never-ran",
                f"neither subscription ran in 28 s, so this run says nothing about clock jumps: next_due on disk {dues}",
            )
            if not overdue_runs:
                return

            # Coalesce. 28,800 periods are missed; a storm is orders of magnitude from the cadence,
            # so the bound is generous on purpose and still names the harm.
            cx.rec.expect(
                overdue_runs <= 3,
                "ck-01-overdue-subscription-fired-once-per-missed-period",
                f"a 30-second subscription ten days overdue ran {overdue_runs} times in 28 s. It is {cx.rec.notes['periods_missed']} periods behind, and running the backlog means that many commands — for a timer subscription, that many model turns and their cost",
                "arbos-kernel subscriptions: a due time in the past coalesces to one run, as the plan engine's cron-coalesce did",
            )

            # Rewind recovery: it either fired, or its due time was pulled back to somewhere reachable.
            future_due = next((v for name, v in dues.items() if "future" in name), None)
            pulled_back = False
            if future_due:
                try:
                    pulled_back = (datetime.datetime.strptime(future_due, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=datetime.timezone.utc) - now) <= datetime.timedelta(days=1)
                except (ValueError, TypeError):
                    pulled_back = False
            cx.rec.notes["future_due_pulled_back"] = pulled_back
            cx.rec.expect(
                future_runs > 0 or pulled_back,
                "ck-01-subscription-stranded-in-the-future-by-a-clock-rewind",
                f"a subscription whose next_due is ten days ahead — which is what a clock set backwards leaves — did not run in 28 s and its due time on disk is still {future_due!r}. It will not run for ten days, and nothing says so",
                "arbos-kernel subscriptions: a due time further ahead than one period is a rewound clock, not a schedule (the plan engine's cron-clock-rewind)",
            )
        finally:
            k.stop()
        cx.check()

    # ── after a crash mid-append: the partial line nobody repairs ──
    @reg("pl-01-a-turn-after-a-crash-mid-append-is-not-swallowed-by-the-partial-line", tags=("after-failure", "transcript", "destructive-order"))
    def pl01(cx):
        """`append_events` writes one `O_APPEND` call ending in a newline, and when that write fails
        part-way it calls `drop_partial_line` to cut the headless line back off. Its own comment names
        the hazard: *"A write that failed part-way (disk full, size limit) leaves the head of a line
        with no newline. Every reader skips it, but it also swallows"* what comes next.

        That guard runs in the process whose write failed. A **crash** — SIGKILL, a lost machine, a
        power cut — leaves the same partial line with nobody to run it: `drop_partial_line` has
        exactly one caller (`files.rs:581`, the failed-write path) and nothing repairs a transcript at
        startup. So the next kernel appends its first event onto the partial line, `load_transcript`
        skips the combined line as unparseable (`files.rs:629`, `if let Ok(...)` with no else), and
        that event is gone with nothing said.

        Staged as a crash leaves it: three whole events, then a fourth line cut off mid-string with no
        trailing newline. Then a turn runs. The property is that a person's next words survive a crash
        that happened before they typed them."""
        place = cx.place
        place.mkdir(parents=True, exist_ok=True)
        plain_agent(place)

        tpath = place / ".arbos" / "agents" / "root" / "transcript.jsonl"
        tpath.parent.mkdir(parents=True, exist_ok=True)
        whole = [
            {"ts": now_ms() - 3000, "kind": "wake", "wake": "user", "text": "one"},
            {"ts": now_ms() - 2000, "kind": "assistant", "text": "first"},
            {"ts": now_ms() - 1000, "kind": "turn_complete"},
        ]
        with open(tpath, "w") as f:
            for e in whole:
                f.write(json.dumps(e) + "\n")
            # The crash: a line begun and never finished. No trailing newline is the whole point.
            f.write('{"ts": %d, "kind": "assistant", "text": "hal' % now_ms())

        before_bytes = tpath.stat().st_size

        def read_lines():
            good, bad = [], []
            for line in tpath.read_text(errors="replace").split("\n"):
                if not line.strip():
                    continue
                try:
                    good.append(json.loads(line))
                except ValueError:
                    bad.append(line)
            return good, bad

        good0, bad0 = read_lines()
        k = cx.kernel(extra_args=["--provider", "replay", "--replies", str(replies_file(cx, [{"agent": "root", "content": "SURVIVED"}]))])
        try:
            started = k.start()
            cx.rec.expect(started, "pl-01-kernel-did-not-start", "the kernel did not come up on a transcript whose last line is partial; a crash must not make a place unservable")
            if not started:
                return
            c = k.attach()
            c.wait(lambda f: f.get("type") == "snapshot", 5)
            marker = f"AFTER-CRASH-{now_ms() % 100000}"
            c.user("root", f"{marker}: reply with the single word SURVIVED.")
            wait_turn_complete(c, "root", 60)

            good1, bad1 = read_lines()
            typed = [e for e in good1 if marker in json.dumps(e)]
            swallowed = [b for b in bad1 if marker in b]
            cx.rec.notes.update({
                "readable_before": len(good0),
                "unparseable_before": len(bad0),
                "readable_after": len(good1),
                "unparseable_after": len(bad1),
                "typed_line_readable": bool(typed),
                "typed_line_inside_an_unparseable_line": bool(swallowed),
                "unparseable_tail": [b[:160] for b in bad1][:2],
                "bytes_before": before_bytes,
                "bytes_after": tpath.stat().st_size,
            })

            # Probe validity: the staged line must really be unreadable, or there is no crash here.
            cx.rec.expect(
                len(bad0) == 1 and len(good0) == len(whole),
                "probe-partial-line-not-staged",
                f"the staged transcript reads as {len(good0)} whole and {len(bad0)} partial line(s); this run does not stand for a crash mid-append",
            )
            if not (len(bad0) == 1 and len(good0) == len(whole)):
                return

            # What a crash must not cost: the events that were already whole.
            cx.rec.expect(
                len(good1) >= len(good0),
                "pl-01-earlier-events-lost",
                f"the transcript held {len(good0)} readable events before the kernel started and {len(good1)} after; a partial last line must cost nothing that was already whole",
            )
            # And the point. `append_events` writes a batch in one `O_APPEND` call, so the headless
            # line swallows the batch's **first** line and the rest land whole — which is why looking
            # only for the marker somewhere readable passes by luck. The property is that *no* event
            # appended after the crash ends up inside an unparseable line.
            cx.rec.expect(
                not swallowed,
                "pl-01-typed-line-swallowed-by-the-partial-line",
                f"an event appended after the crash is inside an unparseable line, run onto the headless one: {(swallowed[0][:150] if swallowed else '')!r}. "
                f"`append_events` writes a batch in one O_APPEND call, so the headless line takes the batch's first event — here the `wake` — while the rest land whole; every reader skips the combined line, so that event is gone and nothing says so. "
                f"`drop_partial_line` repairs only the process whose own write failed (files.rs:581, its one caller); after a crash nobody runs it",
                "arbos-core files.rs append_events — cut a headless last line before appending, not only when this process's write failed",
            )
            # #646 (`cecd48e1`) added the second call site *and* says what it cut. The saying is half
            # the contract: a record that silently loses 55 bytes is still a record someone has to
            # trust. Measured on `cecd48e1bd76`: "ended in the middle of a line (55 bytes, the head of
            # one event). That half line was dropped so everything from here on reads whole; the event
            # it began was lost with that kernel, not now." The last clause is the one that matters —
            # it puts the loss on the crash rather than on the repair.
            cut_said = [
                str(e.get("text") or "")[:260] for e in good1
                if e.get("kind") == "notice" and "middle of a line" in str(e.get("text") or "").lower()
            ]
            cx.rec.notes["cut_said_on_the_transcript"] = cut_said
            cx.rec.expect(
                bool(cut_said),
                "pl-01-the-cut-is-not-said",
                f"root's transcript does not say that a half-written line was cut at start ({len([e for e in good1 if e.get('kind') == 'notice'])} notice(s)). Either the cut did not happen — and the break above says whether an event was swallowed — or it happened and took bytes out of the record with nothing said, which is a record nobody can account for",
                "arbos-kernel serve.rs repair_headless_tails — say what was dropped (#646)",
            )
        finally:
            k.stop()
        # No `cx.check()`: the partial line is this scenario's fixture, so the place checker's
        # `state:transcript-bad-lines` would fire on the thing being staged and bury the verdict.
        # That rule is how this gap was found — it detects the residue and nothing created it.

    # ── #453: the app's swap leaves a gap, and a gap is not a failed restart ──
    @reg("up-01-a-swap-still-in-progress-is-not-a-failed-restart", tags=("after-failure", "update", "destructive-order"))
    def up01(cx):
        """#453 (`b0b3cf841c77`). The app's swap is a directory rename and then a copy into the start
        path, so for a moment the path holds nothing usable. Before the fix the kernel read that as a
        failed restart: measured here on `arbos-kernel 0.2.0 42cb9751ace8 protocol 1` (the commit
        before it), the old build **attempts the exec onto a zero-byte file** — one `reexec` line,
        "restarting onto …" — and then waits `REEXEC_RETRY_MS`, a full minute, before looking again.
        The PR describes the minute; the staged gap shows the attempt that earns it.

        After the fix the file must be whole and at rest — same size and mtime across a 250 ms pause,
        non-empty — and a file that is not is `NotReady`, worth `REEXEC_LOOK_AGAIN_MS` (2 s) rather
        than sixty. The PR could not reproduce the original red (0/12 either way) and rests on a
        reading of the code, so this stages the window deterministically instead.

        Held open the way the app makes it: rename the running image aside — an in-place truncate is
        impossible on a live binary (ETXTBSY), which is *why* the app renames and why the gap exists
        — and leave an empty file at the start path."""
        root = cx.scratch / "up01"
        binpath = root / "bin" / "arbos-kernel"
        place = root / "place"
        (root / "bin").mkdir(parents=True, exist_ok=True)
        place.mkdir(parents=True, exist_ok=True)
        binpath.write_bytes(Path(cx.binary).read_bytes())
        os.chmod(binpath, 0o755)

        # The harness starts `cx.binary`, and this scenario needs the copy it can rename out from
        # under a running kernel. Swapping the context's binary for the duration keeps the kernel
        # inside ns-wrap, which is not negotiable, instead of re-implementing the launch here.
        was = cx.binary
        cx.binary = str(binpath)
        k = None
        try:
            k = cx.kernel(place=place)
            started = k.start()
        finally:
            cx.binary = was
        try:
            cx.rec.expect(started, "up-01-kernel-did-not-start", "the kernel did not come up from the copied binary, so the swap window was never staged")
            if not started:
                return

            klog = place / ".arbos" / "runtime" / "kernel.log"
            time.sleep(2)
            binpath.replace(binpath.with_suffix(".old"))
            binpath.write_bytes(b"")
            os.chmod(binpath, 0o755)
            opened = time.time()
            time.sleep(30)

            events = []
            for line in klog.read_text(errors="replace").splitlines():
                line = line.strip()
                if not line:
                    continue
                try:
                    events.append(json.loads(line))
                except ValueError:
                    continue
            said = [e for e in events if e.get("event") == "binary_gone"]
            waits = [e for e in events if e.get("event") == "reexec_wait"]
            tried = [e for e in events if e.get("event") == "reexec"]
            cx.rec.notes.update({
                "binary_gone_said": len(said),
                "reexec_wait_lines": len(waits),
                "reexec_attempts": len(tried),
                "watched_s": round(time.time() - opened, 1),
                "attempt_detail": [str(e.get("detail") or "")[:120] for e in tried][:2],
            })

            # Probe validity: if it never noticed, the arms below say nothing.
            cx.rec.expect(
                bool(said),
                "probe-binary-gone-never-noticed",
                f"the kernel never said `binary_gone` after its image was renamed aside, so the swap window was not staged: {len(events)} klog events, {sorted({e.get('event') for e in events})}",
            )
            if not said:
                return
            cx.rec.expect(
                not tried,
                "up-01-exec-attempted-onto-an-unusable-file",
                f"the kernel tried to exec onto the start path while it held nothing usable ({cx.rec.notes['attempt_detail']}); an empty or half-written file must be waited for, not jumped onto",
                "arbos-kernel serve.rs reexec_onto_new_binary — the settled check (#453)",
            )
            cx.rec.expect(
                len(waits) >= 2,
                "up-01-waits-a-minute-on-a-swap-in-progress",
                f"only {len(waits)} `reexec_wait` line(s) in {cx.rec.notes['watched_s']} s: the kernel is treating a swap still in progress as a failed restart and waiting REEXEC_RETRY_MS (60 s) rather than looking again in 2 s",
                "arbos-kernel serve.rs: Reexec::NotReady must earn REEXEC_LOOK_AGAIN_MS, not REEXEC_RETRY_MS",
            )
        finally:
            if k is not None:
                k.stop()

    # ── #446: which kernel.json names the kernel that is actually there ──
    @reg("fm-03-the-kernel-record-that-names-a-live-process-wins-whichever-path-it-is-in", tags=("first-match", "place", "lock"))
    def fm03(cx):
        """#446 (`cb495a38e189`). A place has two kernel records — `.arbos/kernel.json` from before the
        `runtime/` split and `.arbos/runtime/kernel.json` after it — and they can both exist and
        disagree, because they have different writers. `runtime/` was read first, so a place that had
        *ever* been served by a newer kernel kept a `runtime/` file for ever; if an older kernel then
        served it, the reader took the stale record, found its pid gone, and reported **no kernel at
        all**. An empty-looking place is precisely the state in which something starts a second
        kernel on it, which is `#450`'s harm arriving by another route.

        The fix asks "which of these names a process that is still there" instead of "which path is
        newer", and falls back to the newer path when neither does. Three arms pin that whole table,
        so a regression in either direction fails: the reader is observed through
        `arbos-kernel feedback <place>`, whose `kernel.serving` block is built by
        `place.kernel_json_read()` — the reader itself (`feedback_cmd.rs:78`)."""
        root = cx.scratch / "fm03"
        live = subprocess.Popen(["sleep", "600"])
        dead = subprocess.Popen(["sleep", "0.05"])
        dead.wait()
        dead_pid, live_pid = dead.pid, live.pid

        def place(name, runtime_pid, runtime_sha, legacy_pid, legacy_sha):
            p = root / name
            (p / ".arbos" / "runtime").mkdir(parents=True, exist_ok=True)
            (p / ".arbos" / "agents" / "root").mkdir(parents=True, exist_ok=True)
            (p / ".arbos" / "agents" / "root" / "agent.md").write_text("root\n")
            (p / ".arbos" / "agents" / "root" / "transcript.jsonl").write_text(
                json.dumps({"ts": 1000, "kind": "wake", "wake": "user", "text": "go"}) + "\n"
                + json.dumps({"ts": 1100, "kind": "turn_complete"}) + "\n"
            )
            rec = lambda pid, sha: json.dumps({"url": "tcp://127.0.0.1:44471", "auth": "loopback", "pid": pid, "started": now_ms(), "version": "0.2.0", "git_sha": sha, "log": "x"}) + "\n"
            (p / ".arbos" / "runtime" / "kernel.json").write_text(rec(runtime_pid, runtime_sha))
            (p / ".arbos" / "kernel.json").write_text(rec(legacy_pid, legacy_sha))
            return p

        def chosen(p):
            """What the reader picked, through the CLI that asks it. `feedback` needs an agent named
            root, which is why each place has one."""
            out = subprocess.run([cx.binary, "feedback", str(p)], capture_output=True, text=True, timeout=60)
            try:
                return ((json.loads(out.stdout) or {}).get("kernel") or {}).get("serving") or {}
            except (json.JSONDecodeError, ValueError):
                return {"parse_error": ((out.stdout or "") + (out.stderr or ""))[:200]}

        try:
            arms = {
                # the case measured on the target: the preferred path is stale, the fallback is live
                "a_runtime_dead_legacy_live": (place("a", dead_pid, "deadf00d0000", live_pid, "1iveliveaaaa"), "1iveliveaaaa"),
                # no regression: when the newer path is the live one it must still win
                "b_runtime_live_legacy_dead": (place("b", live_pid, "1iveliveaaaa", dead_pid, "deadf00d0000"), "1iveliveaaaa"),
                # neither is live: the newer path wins exactly as before, so "nothing serving" is unchanged
                "c_neither_live": (place("c", dead_pid, "deadf00d0000", dead_pid, "0ldand0ld000"), "deadf00d0000"),
            }
            seen = {}
            for name, (p, want_sha) in arms.items():
                got = chosen(p)
                seen[name] = {"want_sha": want_sha, "got_sha": got.get("git_sha"), "live": got.get("live"), "known": got.get("known"), "parse_error": got.get("parse_error")}
            cx.rec.notes.update({"arms": seen, "live_pid": live_pid, "dead_pid": dead_pid})

            # Probe validity first: if `feedback` cannot be asked, the arms say nothing.
            broken = [n for n, v in seen.items() if v["parse_error"]]
            cx.rec.expect(
                not broken,
                "probe-feedback-unreadable",
                f"`arbos-kernel feedback` gave no readable JSON for {broken}, so the reader was never asked: {[seen[n]['parse_error'] for n in broken][:1]}",
            )
            if broken:
                return
            for name, (p, want_sha) in arms.items():
                got = seen[name]
                cx.rec.expect(
                    got["got_sha"] == want_sha,
                    f"fm-03-wrong-record-{name}",
                    f"{name.replace('_', ' ')}: the reader took `{got['got_sha']}` where `{want_sha}` is the record that names a live process"
                    + (". A dead record in the preferred path makes the place read as empty, and an empty place is what a second kernel starts on" if name.startswith("a_") else ""),
                    "arbos-core place.rs kernel_json_read / names_a_live_pid (#446)",
                )
        finally:
            try:
                live.send_signal(signal.SIGKILL)
                live.wait(timeout=10)
            except (OSError, subprocess.SubprocessError):
                pass

    # ── first-match readers: the first location fails and the next one takes the name ──
    # fm-01 (the checkpoint sidecar) is in landing_scenarios.py; this is the same family, found by
    # walking the kernel's first-match readers rather than by a break. The audit listed six — the
    # history lookup, the checkpoint sidecar, the roster files, the leash pointer, the legacy
    # kernel.json and the held-record loader. `mcp::load_servers` is a seventh nobody had checked.
    @reg("fm-02-a-place-mcp-file-that-does-not-parse-is-said-and-does-not-hand-its-name-away", needs_model=False, tags=("first-match", "mcp", "config"))
    def fm02(cx):
        """`mcp::load` walks four locations in order — `.arbos/mcp.toml`, `.cursor/mcp.json`,
        `.mcp.json`, `$XDG_CONFIG_HOME/arbos/mcp.toml` — and the first file to define a server name
        keeps it. `qal-j31`: a file that did not parse was skipped with an `eprintln` and the walk went
        on, so one wrong character in the place's own config handed that server's name to whatever came
        next, and the only record went to a log the window never shows.

        Fixed in #613 (`23ef527c`, on `main` since 09:34 UTC): a place file that fails to parse is said
        as a notice on root's transcript, and it **blocks the machine's file** so the name cannot be
        served by the machine's server of the same name.

        Two things are asserted, both #613's contract, both on `main`:
          1. the notice is there, and names the file and the parse error;
          2. no server of that name starts when only the machine's file offers one.

        The third — a later *place* file (`.cursor/mcp.json`, `.mcp.json`) must not hand the name away
        either — is #644, still open at 12:45 UTC and reproduced here. Rather than stand a red for an
        open PR, the check gates itself on the product's own claim: #644 makes the notice say no later
        file was read, so once it says that, a started server contradicts it and is a break. Until
        then the residual is recorded as a note."""
        place = cx.place
        place.mkdir(parents=True, exist_ok=True)
        plain_agent(place)

        arbos = place / ".arbos"
        arbos.mkdir(parents=True, exist_ok=True)
        # The features agent's own shape: one unclosed array.
        broken = arbos / "mcp.toml"
        broken.write_text('[servers.notes]\ncommand = "notes-mcp"\nargs = [\n')

        # The machine's file, valid, naming the same server. #613 must not let this be used.
        cfg = cx.scratch / "xdg"
        (cfg / "arbos").mkdir(parents=True, exist_ok=True)
        (cfg / "arbos" / "mcp.toml").write_text('[servers.notes]\ncommand = "/bin/true"\nargs = ["--from-the-machine"]\n')
        cx.env["XDG_CONFIG_HOME"] = str(cfg)

        # A later *place* file offering the same name: #644's territory.
        (place / ".cursor").mkdir(parents=True, exist_ok=True)
        (place / ".cursor" / "mcp.json").write_text(json.dumps({"mcpServers": {"notes": {"command": "/bin/true", "args": ["--from-dot-cursor"]}}}) + "\n")

        k = cx.kernel(extra_args=["--provider", "replay", "--replies", str(replies_file(cx, [{"agent": "root", "content": "Up."}]))])
        try:
            started = k.start()
            cx.rec.expect(started, "fm-02-kernel-did-not-start", "the kernel did not come up with an unparseable .arbos/mcp.toml; a broken config must not stop it serving")
            if not started:
                return
            c = k.attach()
            c.wait(lambda f: f.get("type") == "snapshot", 5)
            c.user("root", "Say up.")
            wait_turn_complete(c, "root", 60)

            stderr_text = k.stderr_text() or ""
            # A server of that name being started at all is the takeover, whichever file offered it:
            # the kernel names it on stderr as `mcp: notes: …` when it talks to it.
            took_the_name = [l for l in stderr_text.splitlines() if l.strip().startswith("mcp: notes:")]

            evs, _ = transcript(place, "root")
            said = [e for e in notices(evs) if str(e.get("text") or "").strip().startswith("MCP:")]
            # What the notice claims about files after the broken one. #644's wording is "no MCP
            # file after it was read … not the place's other files, not the machine's own"; the
            # sentence before it named only the machine's file.
            names_the_later_files = any(w in str(e.get("text") or "").lower() for e in said for w in ("after it", "other files"))
            cx.rec.notes.update({
                "mcp_notices": [str(e.get("text") or "")[:260] for e in said],
                "server_of_that_name_started": [l[:160] for l in took_the_name][:2],
                "notice_names_the_files_after_it": names_the_later_files,
                "kernel_still_answering": not c.closed,
            })

            # Probe validity: the file must really have failed to parse, or this run stands for nothing.
            rejected = [l for l in stderr_text.splitlines() if "mcp.toml" in l and "parse" in l.lower()]
            cx.rec.expect(
                bool(rejected) or bool(said),
                "probe-config-was-not-rejected",
                f"nothing names a parse failure for `.arbos/mcp.toml`, so the file may have parsed and this run does not stage the fault it claims: {stderr_text[-200:]!r}",
            )
            if not (rejected or said):
                return

            # 1. #613's telling.
            cx.rec.expect(
                bool(said),
                "fm-02-broken-place-config-is-silent-to-the-user",
                f"`.arbos/mcp.toml` does not parse and root's transcript carries no `MCP:` notice ({len(rejected)} line(s) went to the kernel's stderr instead). The desktop routes stderr to `.arbos/runtime/kernel.out.log`, which the window never shows, so its author is told nothing",
                "arbos-kernel mcp.rs load — say it on root's transcript (#613)",
            )
            if said:
                text = str(said[-1].get("text") or "")
                cx.rec.expect(
                    "does not parse" in text and "mcp.toml" in text,
                    "fm-02-notice-names-neither-the-file-nor-the-fault",
                    f"the notice does not name the file and what is wrong with it: {text[:200]!r}",
                )

            # 2. The walk stops at the first broken place file: no file after it is read, neither the
            # place's own later ones nor the machine's. #613 blocked the machine's file; #644
            # (`f97bb348`, on `main`) stops the walk there. Both are merged, so this is a plain
            # assertion — it was gated on the notice's wording only while #644 was open.
            cx.rec.expect(
                not took_the_name,
                "fm-02-a-file-after-the-broken-one-handed-the-name-away",
                f"`.arbos/mcp.toml` does not parse and a `notes` server was started anyway: {took_the_name[:1]}. "
                f"A file after the broken one — `.cursor/mcp.json` here, or the machine's own — gave that name a different server, so the person is told its servers are off while something else answers to it",
                "arbos-kernel mcp.rs load_from — a place file that does not parse stops the walk there (#613, #644)",
            )
            # And the notice must describe what it did, not less. #644 widened it from "the machine's
            # own MCP file was not used" to "no MCP file after it was read … not the place's other
            # files, not the machine's own" — a person who reads the narrower sentence can still
            # believe a later place file took over.
            if said:
                text = str(said[-1].get("text") or "").lower()
                cx.rec.expect(
                    "after it" in text or "other files" in text,
                    "fm-02-notice-describes-less-than-it-did",
                    f"the notice names only the machine's file, so it under-describes the block it actually performs: {str(said[-1].get('text') or '')[:200]!r}",
                    "arbos-kernel mcp.rs problem_notice (#644)",
                )
        finally:
            cx.env.pop("XDG_CONFIG_HOME", None)
            k.stop()
        cx.check()

    return reg

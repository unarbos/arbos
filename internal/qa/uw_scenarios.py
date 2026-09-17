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

import json
import os
import subprocess
import threading
import time
from pathlib import Path


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
                f"{caught} job folder(s) could not take their marker; {len(leftover)} process(es) still run with nothing to place them (pids {[l.split()[0] for l in leftover][:4]}), {len([u for u in unplaceable if u['exit_file']])} of them already finished with output nobody can attribute; nothing was said on the transcript. A restart now finds a job it cannot place and the run's result reaches nobody",
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

    return reg

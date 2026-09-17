"""Batches 44/45 on main (2026-09-15): remote kernel install/update over ssh (#229, #232), the
kickoff turn on a fresh place (#228, #232), the project face in hello and the hub roster (#233),
and the attach port's answer to a plain GET (#234).

The "remote machine" is this box over loopback ssh (`ubuntu@127.0.0.1`, the agents' key), so the
install path runs for real: probe, scp of this kernel as <bin>.new + mv, place sync, remote
serve, child turn, report. Skipped when loopback ssh is not set up (ARBOS_QA_SSH_KEY unset or
sshd not answering).
"""

import hashlib
import json
import os
import shutil
import socket
import subprocess
import time
import urllib.request
from pathlib import Path

SSH_KEY = os.environ.get("ARBOS_QA_SSH_KEY", os.path.expanduser("~/.ssh/arbos_agents"))
SSH_USER = os.environ.get("ARBOS_QA_SSH_USER", os.environ.get("USER", "ubuntu"))


def loopback_ssh_ok():
    if not Path(SSH_KEY).exists():
        return False
    r = subprocess.run(["ssh", "-i", SSH_KEY, "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=accept-new", "-o", "ConnectTimeout=5", f"{SSH_USER}@127.0.0.1", "true"], capture_output=True, timeout=20)
    return r.returncode == 0


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest() if Path(path).exists() else None


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


def register(scenario, registry, transcript, now_ms, model_turn, branch):
    def reg(name, needs_model=False, tags=()):
        def deco(fn):
            scenario(name, needs_model=needs_model, tags=("batch-44-45",) + tuple(tags))(fn)
            registry[name]["branch"] = branch
            return fn

        return deco

    def machines_toml(cx, remote_dir, name="loop"):
        cfg = cx.scratch / "xdg" / "arbos"
        (cfg / "machines.toml").write_text(f'[[machine]]\nname = "{name}"\nssh = "{SSH_USER}@127.0.0.1"\ndir = "{remote_dir}"\nkey = "{SSH_KEY}"\ntags = ["linux", "x86_64", "loopback"]\nnote = "this box over loopback ssh (QA)"\n')
        # ssh from the scenario's scratch HOME needs a known_hosts it may write.
        (cx.scratch / "home" / ".ssh").mkdir(parents=True, exist_ok=True)

    def start(cx):
        k = cx.kernel()
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        c = k.attach()
        hello = c.wait(lambda f: f.get("type") == "hello", 5, "hello")
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        return k, c, hello

    # ── #229 remote install over ssh ─────────────────────────────────────

    @reg("rm-01-remote-install-and-spawn", needs_model=True, tags=("remote",))
    def s01(cx):
        """spawn host=loop on a machine with no kernel: the kernel is installed over ssh (this build, scp as .new + mv), the place synced, a remote worker runs and reports."""
        if not loopback_ssh_ok():
            cx.rec.notes["skipped"] = "no loopback ssh (ARBOS_QA_SSH_KEY / sshd)"
            return
        remote_dir = f"/tmp/arbos-qa-remote-{now_ms()}"
        machines_toml(cx, remote_dir)
        k, c, evs = model_turn(cx, "Spawn one worker on machine `loop` (host=loop, wait=true, wait_secs=240) whose task is to run `hostname; whoami; pwd` with bash and report the three lines. Then tell me exactly what it reported.", timeout=420)
        spawns = [e for e in evs if e.get("kind") == "tool" and e.get("name") == "spawn"]
        errs = [str(e.get("error"))[:200] for e in spawns if e.get("error")]
        kernel_bin = Path(remote_dir) / "bin" / "arbos-kernel"
        installed = kernel_bin.exists()
        same_build = installed and sha256(kernel_bin) == sha256(cx.binary)
        remotes = cx.place / ".arbos" / "remotes.json"
        rtext = remotes.read_text(errors="replace") if remotes.exists() else ""
        text = " ".join(e.get("text", "") for e in evs if e.get("kind") == "assistant")
        cx.rec.notes.update({"spawn_errors": errs, "installed": installed, "same_build": same_build, "remotes_json": rtext[:300], "remote_dir_listing": sorted(p.name for p in Path(remote_dir).iterdir()) if Path(remote_dir).exists() else []})
        cx.rec.expect(spawns and not errs, "rm-01-spawn-refused", f"remote spawn refused: {errs[:2]}", "arbos-kernel remote.rs prepare_remote / spawn_ssh (#229)")
        cx.rec.expect(installed, "rm-01-kernel-not-installed", f"no arbos-kernel at {kernel_bin} after a spawn on a machine without one")
        cx.rec.expect(same_build, "rm-01-wrong-build-installed", "the installed kernel is not this build (same arch: the local binary is what gets copied)")
        says = [e.get("text", "") for e in evs if e.get("kind") == "say"]
        reported = any(socket.gethostname() in t or SSH_USER in t for t in says + [text])
        cx.rec.expect(reported, "rm-01-no-report", f"the worker's report (hostname/user) reached neither root's reply nor a say line: {text[-200:]!r}")
        # spawn wait=true must wait for the report, not for the first notice the remote sends.
        in_reply = socket.gethostname() in text or SSH_USER in text
        cx.rec.expect(in_reply or not reported, "rm-01-wait-returned-early", "spawn wait=true returned before the remote worker's report (root replied on the sync notices; the report arrived later as a say)", "remote.rs spawn_ssh: the wait ends on the first relayed message (qa-037)")
        cx.rec.expect(remotes.exists() and "loop" in rtext, "rm-01-no-remote-record", "no remotes.json record for the ssh child")
        k.stop()
        time.sleep(2)
        left = subprocess.run(["pgrep", "-af", remote_dir], capture_output=True, text=True).stdout.strip().splitlines()
        cx.rec.notes["remote_processes_after_stop"] = left[:4]
        cx.rec.expect(not [l for l in left if "arbos-kernel" in l], "rm-01-remote-kernel-left-running", f"the remote kernel outlived the parent: {left[:2]}", "#229 safe restart / leash (qa-038)")
        subprocess.run(["pkill", "-TERM", "-f", f"^{remote_dir}/bin/arbos-kernel"], capture_output=True)
        shutil.rmtree(remote_dir, ignore_errors=True)
        cx.check()

    @reg("rm-02-remote-update-replaces-older-build", needs_model=True, tags=("remote",))
    def s02(cx):
        """The machine already has an arbos-kernel of another build: the version rule replaces it (staged as .new, moved into place, one pid), and the spawn still works."""
        if not loopback_ssh_ok():
            cx.rec.notes["skipped"] = "no loopback ssh"
            return
        other = os.environ.get("ARBOS_QA_OTHER_KERNEL", "/workspace/target/debug/arbos-kernel")
        if not Path(other).exists():
            cx.rec.notes["skipped"] = f"no other build at {other} (ARBOS_QA_OTHER_KERNEL)"
            return
        remote_dir = f"/tmp/arbos-qa-remote-{now_ms()}"
        (Path(remote_dir) / "bin").mkdir(parents=True)
        shutil.copy2(other, Path(remote_dir) / "bin" / "arbos-kernel")
        planted = sha256(Path(remote_dir) / "bin" / "arbos-kernel")
        machines_toml(cx, remote_dir)
        k, c, evs = model_turn(cx, "Spawn one worker on machine `loop` (host=loop, wait=true, wait_secs=240) whose task is to run `echo UPDATED-OK` with bash and report it. Tell me what it said.", timeout=420)
        spawns = [e for e in evs if e.get("kind") == "tool" and e.get("name") == "spawn"]
        errs = [str(e.get("error"))[:200] for e in spawns if e.get("error")]
        now_sha = sha256(Path(remote_dir) / "bin" / "arbos-kernel")
        leftovers = sorted(p.name for p in (Path(remote_dir) / "bin").iterdir())
        klog = (cx.place / ".arbos" / "runtime" / "kernel.log")
        ktext = klog.read_text(errors="replace") if klog.exists() else ""
        cx.rec.notes.update({"spawn_errors": errs, "replaced": now_sha != planted, "is_this_build": now_sha == sha256(cx.binary), "bin_dir": leftovers, "remote_log_lines": [l[:160] for l in ktext.splitlines() if "remote" in l.lower() or "install" in l.lower()][:6]})
        cx.rec.expect(spawns and not errs, "rm-02-spawn-refused", f"remote spawn refused: {errs[:2]}")
        cx.rec.expect(now_sha != planted and now_sha == sha256(cx.binary), "rm-02-not-updated", "an older/other build at <dir>/bin/arbos-kernel was not replaced by this build (version rule, #229)", "remote.rs prepare_remote version rule")
        cx.rec.expect(not [n for n in leftovers if n.endswith(".new")], "rm-02-staged-file-left", f"a staged .new binary was left behind: {leftovers}")
        text = " ".join(e.get("text", "") for e in evs if e.get("kind") == "assistant")
        cx.rec.expect("UPDATED-OK" in text, "rm-02-no-report", "the worker's report did not reach root")
        k.stop()
        subprocess.run(["pkill", "-TERM", "-f", f"^{remote_dir}/bin/arbos-kernel"], capture_output=True)
        shutil.rmtree(remote_dir, ignore_errors=True)
        cx.check()

    # ── #228/#232 the kickoff turn ──────────────────────────────────────

    @reg("rm-03-kickoff-turn-fresh-place", needs_model=True, tags=("kickoff-turn",))
    def s03(cx):
        """A fresh place's first open asks the kernel for the kickoff turn: one `kickoff` wake, a bounded turn that reads the store and speaks; a second `kickoff` frame is a no-op; a keyless attempt does not count as taken."""
        # Part 1: no key — the kickoff turn fails; it must not count as taken.
        key = cx.env.pop("OPENROUTER_API_KEY", None)
        k, c, hello = start(cx)
        c.send({"type": "kickoff", "agent": "root"})
        c.wait_turn("root", "idle", 30)
        time.sleep(1)
        evs, _ = transcript(cx.place, "root")
        keyless = [e for e in evs if e.get("kind") == "wake" and e.get("wake") == "kickoff"]
        cx.rec.notes["keyless_kickoff_wakes"] = len(keyless)
        k.stop()
        # Part 2: with a key — the kickoff turn runs once.
        if key:
            cx.env["OPENROUTER_API_KEY"] = key
        k2 = cx.kernel(tag="kernel-keyed")
        cx.rec.expect(k2.start(), "kernel-restart", "kernel did not restart")
        c2 = k2.attach()
        c2.wait(lambda f: f.get("type") == "snapshot", 5)
        c2.send({"type": "kickoff", "agent": "root"})
        started = c2.wait_turn("root", "running", 20)
        cx.rec.expect(started is not None, "rm-03-keyless-counted-as-taken", "after a keyless kickoff attempt, the keyed kickoff did not start a turn (kickoff_taken counted the failed one; #232)", "arbos-kernel hooks.rs kickoff_taken")
        cx.rec.expect(c2.wait_turn("root", "idle", 240) is not None, "turn-never-ended", "the kickoff turn never ended")
        time.sleep(1)
        evs, _ = transcript(cx.place, "root")
        wakes = [e for e in evs if e.get("kind") == "wake" and e.get("wake") == "kickoff"]
        said = [e.get("text", "") for e in evs if e.get("kind") == "assistant" and e.get("text", "").strip()]
        users = [e for e in evs if e.get("kind") == "user"]
        cx.rec.notes.update({"kickoff_wakes": len(wakes), "assistant_lines": len(said), "first_words": (said[0][:200] if said else ""), "user_lines": len(users)})
        cx.rec.expect(wakes, "rm-03-no-kickoff-wake", "no `wake kickoff` on the transcript after the kickoff frame")
        cx.rec.expect(said, "rm-03-silent-kickoff", "the kickoff turn said nothing")
        cx.rec.expect(not users, "rm-03-kickoff-as-user-line", "the kickoff appears as a user line (the user typed nothing)")
        # A second frame: no-op.
        c2.mark = len(c2.frames)
        c2.send({"type": "kickoff", "agent": "root"})
        again = c2.wait_turn("root", "running", 8)
        cx.rec.expect(again is None, "rm-03-kickoff-twice", "a second kickoff frame started another turn")
        # Kickoff frames for a chat that already has history: no-op too.
        c2.user("root", "Reply with the single word HELLO.")
        c2.wait_turn("root", "idle", 120)
        c2.mark = len(c2.frames)
        c2.send({"type": "kickoff", "agent": "root"})
        cx.rec.expect(c2.wait_turn("root", "running", 8) is None, "rm-03-kickoff-after-history", "a kickoff frame on a chat with history started a turn")
        k2.stop()
        cx.check()

    # ── #233 the face ───────────────────────────────────────────────────

    @reg("rm-04-project-face-in-hello-and-roster", tags=("face",))
    def s04(cx):
        """project.toml's top-level name/icon/color ride on `hello` and in the hub roster; a rewrite is announced (`changed project.toml`) and the next hello carries the new face."""
        arbos = cx.place / ".arbos"
        arbos.mkdir(exist_ok=True)
        (arbos / "project.toml").write_text('schema = 2\nname = "QA Face"\nicon = "star"\ncolor = "#ff8800"\n\n[root]\nrole = "coordinator"\n')
        # A hub of our own on loopback.
        hub_bin = os.environ.get("ARBOS_QA_HUB_BIN", str(Path(cx.binary).parent / "arbos-hub"))
        hub_port = free_port()
        hub_cfg = cx.scratch / "hub-server.toml"
        hub_cfg.write_text(f'bind = "127.0.0.1:{hub_port}"\n\n[[machine]]\nname = "qa-box"\ntoken = "machine-secret-1-qa-loopback-only"\n\n[[client]]\nname = "qa-client"\ntoken = "client-secret-1-qa-loopback-only"\nrole = "owner"\n')
        hub = None
        if Path(hub_bin).exists():
            hub = subprocess.Popen([hub_bin, "--config", str(hub_cfg), "--bind", f"127.0.0.1:{hub_port}"], stdout=open(cx.rec.dir / "hub.log", "ab"), stderr=subprocess.STDOUT)
            (cx.scratch / "xdg" / "arbos" / "hub.toml").write_text(f'url = "ws://127.0.0.1:{hub_port}"\nmachine = "qa-box"\ntoken = "machine-secret-1-qa-loopback-only"\n')
            time.sleep(1.0)
        else:
            cx.rec.notes["hub"] = f"no arbos-hub binary at {hub_bin}; roster check skipped"
        k, c, hello = start(cx)
        ident = (hello or {}).get("identity") or {}
        cx.rec.notes["hello_identity"] = ident
        cx.rec.expect(ident.get("name") == "QA Face" and ident.get("icon") == "star" and ident.get("color") == "#ff8800", "rm-04-hello-face", f"hello does not carry the face from project.toml: {ident}", "arbos-kernel serve.rs hello identity (#233)")
        if hub:
            face = None
            for _ in range(20):
                try:
                    req = urllib.request.Request(f"http://127.0.0.1:{hub_port}/list", headers={"Authorization": "Bearer client-secret-1-qa-loopback-only"})
                    with urllib.request.urlopen(req, timeout=5) as r:
                        roster = json.load(r)
                    for m in roster.get("machines", []):
                        for p in m.get("projects", []):
                            if p.get("identity"):
                                face = p["identity"]
                    if face:
                        break
                except Exception as e:  # noqa: BLE001
                    cx.rec.notes["roster_error"] = str(e)[:160]
                time.sleep(0.5)
            cx.rec.notes["roster_face"] = face
            cx.rec.expect(face and face.get("name") == "QA Face" and face.get("icon") == "star", "rm-04-roster-face", f"the hub roster carries no face for the registered project: {face}", "arbos-hub hub.rs identities (#233)")
        # #234: a plain GET on the attach port answers, never a closed socket.
        url = json.loads((arbos / "runtime" / "kernel.json").read_text())["url"] if (arbos / "runtime" / "kernel.json").exists() else json.loads((arbos / "kernel.json").read_text())["url"]
        host, port = url.removeprefix("tcp://").rsplit(":", 1)
        code = 0
        try:
            with urllib.request.urlopen(f"http://{host}:{port}/healthz", timeout=5) as r:
                code = r.status
        except urllib.error.HTTPError as e:
            code = e.code
        except Exception as e:  # noqa: BLE001
            cx.rec.notes["get_error"] = str(e)[:120]
        cx.rec.notes["plain_get_healthz"] = code
        cx.rec.expect(code in (200, 426), "rm-04-plain-get-closed", f"a plain GET /healthz on the attach port got {code} (expected 200 or 426, never a closed socket; qa-036/#234)")
        # A rewrite of project.toml: announced, and the next hello has the new face.
        c.mark = len(c.frames)
        (arbos / "project.toml").write_text('schema = 2\nname = "QA Face Two"\nicon = "terminal"\ncolor = "#00aaff"\n\n[root]\nrole = "coordinator"\n')
        changed = c.wait(lambda f: f.get("type") == "changed" and "project.toml" in json.dumps(f), 30, "changed project.toml")
        cx.rec.notes["changed_frame"] = changed
        c2 = k.attach()
        hello2 = c2.wait(lambda f: f.get("type") == "hello", 5, "hello")
        ident2 = (hello2 or {}).get("identity") or {}
        cx.rec.notes["hello_identity_after_rewrite"] = ident2
        cx.rec.expect(changed is not None, "rm-04-rewrite-not-announced", "no `changed project.toml` frame after the file was rewritten")
        cx.rec.expect(ident2.get("name") == "QA Face Two" and ident2.get("icon") == "terminal", "rm-04-stale-face", f"the next hello still carries the old face: {ident2}")
        k.stop()
        if hub:
            hub.terminate()
        cx.check()

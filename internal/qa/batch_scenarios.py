"""Batch on main 43d8569 (2026-09-16): #285 spawn guard reads the brief, #286 worktree re-spawn
takes the next free branch, #287 coordinators see archived workers, #289 `say to=user` refused,
#278 tool markup written as prose is stripped, #270 files arrive as bytes, #272 history pages
backwards, #283 OpenRouter fallbacks with a 403 falling through.

Most run on the replay provider (`serve --provider replay --replies FILE`): the model's replies
are scripted, so the kernel's handling is checked deterministically and for free. `bt-*`.
"""

import http.server
import json
import subprocess
import threading
import time
from pathlib import Path


def replies_file(cx, lines):
    p = cx.scratch / "replies.jsonl"
    p.write_text("".join(json.dumps(l) + "\n" for l in lines))
    return p


def git(place, *args):
    return subprocess.run(["git", "-c", "user.name=qa", "-c", "user.email=qa@qa", *args], cwd=place, capture_output=True, text=True)


def tool_events(evs, name=None):
    return [e for e in evs if e.get("kind") == "tool" and (name is None or e.get("name") == name)]


def result_text(e):
    return str(e.get("error") or e.get("body") or e.get("output") or "")


class FallbackStub(threading.Thread):
    """An OpenAI-compatible endpoint: model `blocked` answers 403 (the OpenRouter policy block),
    model `open` answers with FROM-OPEN. Records the models asked for, in order."""

    def __init__(self):
        super().__init__(daemon=True)
        self.asked = []
        outer = self

        class H(http.server.BaseHTTPRequestHandler):
            def log_message(self, *a):  # noqa: D401
                pass

            def do_GET(self):  # noqa: N802
                body = json.dumps({"data": [{"id": m, "context_length": 32000} for m in ("acme/blocked", "zeta/open", "acme/silent", "zeta/empty")]}).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def do_POST(self):  # noqa: N802
                n = int(self.headers.get("Content-Length") or 0)
                req = json.loads(self.rfile.read(n) or b"{}")
                model = req.get("model", "")
                outer.asked.append(model)
                if model.endswith("/silent"):
                    time.sleep(60)  # no first byte for a minute
                if model.endswith("/blocked"):
                    body = json.dumps({"error": {"message": "Policy Violation: this user has been blocked for a previous policy violation.", "code": 403}}).encode()
                    self.send_response(403)
                elif req.get("stream"):
                    text = "" if model.endswith("/empty") else "FROM-OPEN"
                    chunks = [{"id": "x", "object": "chat.completion.chunk", "model": model, "choices": [{"index": 0, "delta": {"role": "assistant", "content": text}, "finish_reason": None}]},
                              {"id": "x", "object": "chat.completion.chunk", "model": model, "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}}]
                    body = "".join(f"data: {json.dumps(ch)}\n\n" for ch in chunks).encode() + b"data: [DONE]\n\n"
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream")
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                    return
                else:
                    body = json.dumps({"id": "x", "object": "chat.completion", "created": 0, "model": model,
                                       "choices": [{"index": 0, "message": {"role": "assistant", "content": "FROM-OPEN"}, "finish_reason": "stop"}],
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


def register(scenario, registry, transcript, now_ms, model_turn, branch):
    def reg(name, needs_model=False, tags=()):
        def deco(fn):
            scenario(name, needs_model=needs_model, tags=("batch-46",) + tuple(tags))(fn)
            registry[name]["branch"] = branch
            return fn

        return deco

    def replay_turn(cx, lines, prompt, timeout=60, tag="kernel"):
        k = cx.kernel(tag=tag, extra_args=["--provider", "replay", "--replies", str(replies_file(cx, lines))])
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        c = k.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        c.user("root", prompt)
        cx.rec.expect(c.wait_turn("root", "idle", timeout) is not None, "turn-never-ended", "replayed turn never ended")
        time.sleep(0.8)
        evs, _ = transcript(cx.place, "root")
        return k, c, evs

    @reg("bt-01-say-to-user-refused", tags=("say",))
    def s01(cx):
        """#289: `say to=user` is refused with a message that says the user reads the reply; nothing is delivered as a say."""
        k, c, evs = replay_turn(cx, [
            {"agent": "root", "content": "", "calls": [{"name": "say", "arguments": {"to": "user", "text": "Hello via say"}}]},
            {"agent": "root", "content": "The reply itself is what the user reads."},
        ], "Say hello to me with the say tool.")
        says = tool_events(evs, "say")
        cx.rec.notes["say_results"] = [result_text(e)[:200] for e in says]
        cx.rec.expect(says and says[0].get("error"), "bt-01-say-to-user-accepted", f"say to=user was not refused: {cx.rec.notes['say_results']}", "arbos-kernel tools.rs say (#289)")
        cx.rec.expect(not any(e.get("kind") == "say" and "Hello via say" in e.get("text", "") for e in evs), "bt-01-say-delivered", "a say line to `user` landed on the transcript")
        k.stop()
        cx.check()

    @reg("bt-02-spawn-guard-reads-the-brief", tags=("spawn",))
    def s02(cx):
        """#285: a read-only worker whose brief asks to write is refused; a read-only worker with an Output field but a read-only brief is allowed."""
        k, c, evs = replay_turn(cx, [
            {"agent": "root", "content": "", "calls": [
                {"name": "spawn", "arguments": {"name": "writer-ro", "readonly": True, "task": "Write a summary of docs/ into summary.md", "output": "summary.md"}},
                {"name": "spawn", "arguments": {"name": "reader-ro", "readonly": True, "task": "Read docs/ and report the three main points", "output": "report in your last words"}},
            ]},
            {"agent": "root", "content": "spawned"},
            {"content": "three points"},
        ], "Two read-only workers, please.")
        spawns = tool_events(evs, "spawn")
        cx.rec.notes["spawn_results"] = [(json.dumps(e.get("args", {}).get("name")), result_text(e)[:160]) for e in spawns]
        by = {e.get("args", {}).get("name"): e for e in spawns}
        cx.rec.expect(by.get("writer-ro") is not None and by["writer-ro"].get("error"), "bt-02-writing-ro-spawn-allowed", "a read-only worker whose brief writes a file was not refused", "arbos-kernel spawn guard (#285)")
        cx.rec.expect(by.get("reader-ro") is not None and not by["reader-ro"].get("error"), "bt-02-output-field-refused", f"a read-only worker with an Output field but a read-only brief was refused: {result_text(by.get('reader-ro', {}))[:160]}", "#285: judge the brief, not the Output field")
        k.stop()
        cx.check()

    @reg("bt-03-worktree-respawn-next-free-branch", tags=("spawn", "worktree"))
    def s03(cx):
        """#286: spawning the same worker name twice with isolate=worktree takes the next free branch; no refusal, two worktrees."""
        (cx.place / "README.md").write_text("toy\n")
        git(cx.place, "init", "-q", "-b", "main")
        git(cx.place, "add", "-A")
        git(cx.place, "commit", "-q", "-m", "init")
        k, c, evs = replay_turn(cx, [
            {"agent": "root", "content": "", "calls": [{"name": "spawn", "arguments": {"name": "fixer", "isolate": "worktree", "task": "Fix the readme wording"}}]},
            {"agent": "root", "content": "", "calls": [{"name": "spawn", "arguments": {"name": "fixer", "isolate": "worktree", "task": "Fix the readme wording again"}}]},
            {"agent": "root", "content": "two workers"},
            {"content": "done"},
            {"content": "done"},
        ], "Spawn the fixer twice, in worktrees.", timeout=90)
        spawns = tool_events(evs, "spawn")
        bodies = [result_text(e) for e in spawns]
        cx.rec.notes["spawn_results"] = [b[:200] for b in bodies]
        import re as _re
        cut = [_re.findall(r"worktree (\S+) on branch (\S+) \(cut from (\S+)\)", b) for b in bodies]
        wt = [m[0] for m in cut if m]
        cx.rec.notes["worktrees"] = wt
        cx.rec.expect(len(spawns) == 2 and not any(e.get("error") for e in spawns), "bt-03-respawn-refused", f"the second worktree spawn of the same name was refused: {[e.get('error') for e in spawns]}", "arbos-kernel hooks.rs spawn_isolated worktree branch (#286)")
        cx.rec.expect(len(wt) == 2 and wt[0][1] != wt[1][1] and wt[0][0] != wt[1][0], "bt-03-branch-not-advanced", f"two worktree spawns did not yield two branches/worktrees: {wt}")
        k.stop()
        cx.check()

    @reg("bt-04-tool-markup-as-prose-stripped", tags=("markup",))
    def s04(cx):
        """#278: a reply that writes a tool call as text (`<tool_call>{…}</tool_call>`, `<function_calls>…`) does not reach the transcript raw; the kernel strips it and nudges."""
        k, c, evs = replay_turn(cx, [
            {"agent": "root", "content": "Let me list the files. <tool_call>{\"name\": \"bash\", \"arguments\": {\"command\": \"ls\"}}</tool_call>"},
            {"agent": "root", "content": "Here is the listing."},
        ], "List the files.")
        asst = [e.get("text", "") for e in evs if e.get("kind") == "assistant"]
        nudges = [e.get("text", "") for e in evs if e.get("kind") == "nudge"]
        cx.rec.notes.update({"assistant": [a[:120] for a in asst], "nudges": [n[:120] for n in nudges]})
        cx.rec.expect(not any("<tool_call>" in a or "</tool_call>" in a for a in asst), "bt-04-markup-on-transcript", f"raw tool markup reached the transcript: {[a for a in asst if 'tool_call' in a][:1]}", "arbos-engine markup.rs strip_tool_markup (#278)")
        cx.rec.expect(nudges and any("tool call" in n.lower() for n in nudges), "bt-04-no-nudge", f"no `[kernel] that was a tool call written as text` nudge: {nudges}")
        k.stop()
        cx.check()

    @reg("bt-05-history-pages-backwards", tags=("history",))
    def s05(cx):
        """#272: `history limit=N` returns the newest N and `history_end{from,to,total}`; `history before=<from>` returns the page before it, down to the start."""
        root = cx.place / ".arbos" / "agents" / "root"
        root.mkdir(parents=True, exist_ok=True)
        (root / "agent.md").write_text(f"name: root\ntitle: \nparent: \npaused: false\nmodel: inherit\nallowlist: read\nreadonly: false\ncwd: {cx.place}\n")
        lines = []
        t = now_ms() - 100_000
        for i in range(30):
            lines.append(json.dumps({"ts": t + i * 10, "kind": "user", "text": f"line {i}"}))
            lines.append(json.dumps({"ts": t + i * 10 + 1, "kind": "assistant", "text": f"reply {i}"}))
            lines.append(json.dumps({"ts": t + i * 10 + 2, "kind": "turn_complete"}))
        (root / "transcript.jsonl").write_text("\n".join(lines) + "\n")
        k = cx.kernel()
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        c = k.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)

        def page(before=None, limit=10):
            with c.lock:
                start = len(c.frames)
            c.mark = start
            req = {"type": "history", "agent": "root", "limit": limit, "before": before if before is not None else 10**9}
            c.send(req)
            end = c.wait(lambda f: f.get("type") == "history_end" and f.get("agent") == "root", 10, "history_end")
            with c.lock:
                got = [f for _, f in c.frames[start:] if f.get("type") == "replayed" and f.get("agent") == "root"]
            return end, got

        end1, page1 = page()
        cx.rec.notes["page1"] = {"end": end1, "n": len(page1)}
        cx.rec.expect(end1 is not None, "bt-05-no-history-end", "no history_end frame for a history request")
        seqs1 = [f.get("event", {}).get("seq") for f in page1]
        cx.rec.expect(len(page1) == 10 and seqs1 == sorted(seqs1) and max(seqs1 or [0]) == 90, "bt-05-first-page", f"first page: {len(page1)} lines, seqs {seqs1[:3]}…{seqs1[-3:]} (expected the newest 10, ending at 90)")
        end2, page2 = page(before=(end1 or {}).get("from"))
        seqs2 = [f.get("event", {}).get("seq") for f in page2]
        cx.rec.notes["page2"] = {"end": end2, "seqs": seqs2}
        cx.rec.expect(len(page2) == 10 and seqs2 and max(seqs2) < min(seqs1 or [0]), "bt-05-backwards-page", f"the page before {end1 and end1.get('from')} is not the previous 10 lines: {seqs2}", "arbos-kernel serve.rs replay Page::Before (#272)")
        # Walk to the start.
        before = (end2 or {}).get("from")
        hops = 0
        while before and before > 1 and hops < 10:
            endn, pagen = page(before=before)
            if not pagen:
                break
            before = (endn or {}).get("from")
            hops += 1
        cx.rec.notes["hops_to_start"] = hops
        cx.rec.expect(before is not None and before <= 1, "bt-05-never-reaches-start", f"paging backwards stopped at seq {before} after {hops} hops")
        k.stop()
        cx.check()

    @reg("bt-06-fallback-403-falls-through", tags=("provider",))
    def s06(cx):
        """#283: the primary route answers 403 (the policy block); the turn falls through to the fallback model and completes; the transcript says which model answered, not a failed turn."""
        stub = FallbackStub()
        stub.start()
        cfg = cx.scratch / "xdg" / "arbos" / "config.toml"
        lines = [l for l in cfg.read_text().splitlines() if not l.startswith(("api_base", "api_key_env", "model", "fallback_models"))]
        lines += [f'api_base = "{stub.url}"', 'api_key_env = "QA_FAKE_KEY"', 'model = "acme/blocked"', 'fallback_models = ["zeta/open"]']
        cfg.write_text("\n".join(lines) + "\n")
        cx.env["QA_FAKE_KEY"] = "fake"
        try:
            k = cx.kernel()
            cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
            c = k.attach()
            c.wait(lambda f: f.get("type") == "snapshot", 5)
            c.user("root", "Say hello.")
            cx.rec.expect(c.wait_turn("root", "idle", 60) is not None, "turn-never-ended", "turn never ended")
            time.sleep(0.5)
            evs, _ = transcript(cx.place, "root")
            asst = " ".join(e.get("text", "") for e in evs if e.get("kind") == "assistant")
            notices = [e.get("text", "") for e in evs if e.get("kind") == "notice"]
            cx.rec.notes.update({"models_asked": stub.asked, "assistant": asst[:120], "notices": [n[:160] for n in notices]})
            cx.rec.expect(stub.asked[:2] == ["acme/blocked", "zeta/open"], "bt-06-no-fallthrough", f"after a 403 on the primary the kernel did not try the fallback: asked {stub.asked}", "arbos-engine retry.rs / step.rs 403 handling (#283)")
            cx.rec.expect("FROM-OPEN" in asst, "bt-06-turn-failed", f"the turn did not complete on the fallback: {asst!r}; notices {notices[:2]}")
            cx.rec.expect(not any(e.get("kind") == "notice" and e.get("failed") for e in evs), "bt-06-failed-notice", f"a failed notice was written although the fallback answered: {notices[:2]}")
            k.stop()
        finally:
            stub.stop()
        cx.check()

    @reg("bt-07-coordinator-sees-archived-workers", tags=("roster",))
    def s07(cx):
        """#287: after a worker finishes and is archived, root's next prompt still names it (archived, with when/where), so root can answer 'what did the design worker say' without guessing."""
        k, c, evs = replay_turn(cx, [
            {"agent": "root", "content": "", "calls": [{"name": "spawn", "arguments": {"name": "poet", "task": "Write one line about rain and report it"}}]},
            {"agent": "root", "content": "spawned"},
            {"content": "Rain writes on the roof."},
            {"agent": "root", "content": "The poet said: Rain writes on the roof."},
            {"agent": "root", "content": "Still here."},
        ], "Spawn a poet.", timeout=90)
        # Let the worker finish, report, and be archived; then a fresh root turn.
        end = time.time() + 60
        while time.time() < end and not (cx.place / ".arbos" / "archive" / "agents" / "poet").exists():
            time.sleep(1)
        archived = (cx.place / ".arbos" / "archive" / "agents" / "poet").exists()
        c.user("root", "What did the poet say?")
        c.wait_turn("root", "idle", 60)
        time.sleep(0.5)
        traces = sorted((cx.place / ".arbos" / "agents" / "root" / "trace").glob("*.json"))
        last = json.load(open(traces[-1])) if traces else {}
        prompt = json.dumps(last.get("request") or {})
        cx.rec.notes.update({"archived": archived, "traces": len(traces), "mentions_poet": "poet" in prompt, "mentions_archived": "archived" in prompt.lower()})
        cx.rec.expect(archived, "bt-07-not-archived", "the finished worker was not archived within 60 s")
        cx.rec.expect("poet" in prompt and "archived" in prompt.lower(), "bt-07-archived-worker-invisible", "root's prompt after the archive does not name the archived worker", "arbos-kernel hooks.rs roster / prompt archived section (#287)")
        k.stop()
        cx.check()

    @reg("bt-08-files-arrive-as-bytes", tags=("attachments",))
    def s08(cx):
        """#270: a `user` frame with an image attachment reaches the model as image bytes (a data: URL / image part), not as a path string."""
        png = cx.place / "dot.png"
        # A 1x1 PNG.
        png.write_bytes(bytes.fromhex("89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d49444154789c6360000002000154a24f5d0000000049454e44ae426082"))
        k, c, evs = replay_turn(cx, [{"agent": "root", "content": "I see a dot."}], "What is in this image?")
        # The replay turn above had no attachment; now one with it.
        c.send({"type": "user", "agent": "root", "text": "And this one?", "steer": False, "attachments": [str(png)]})
        c.wait_turn("root", "idle", 60)
        time.sleep(0.5)
        traces = sorted((cx.place / ".arbos" / "agents" / "root" / "trace").glob("*.json"))
        last = json.load(open(traces[-1])) if traces else {}
        req = json.dumps(last.get("request") or {})
        cx.rec.notes.update({"traces": len(traces), "has_image_part": "image_url" in req or "data:image" in req or "input_image" in req, "path_as_text": str(png) in req})
        cx.rec.expect(cx.rec.notes["has_image_part"], "bt-08-image-not-bytes", "the attached PNG did not reach the model as an image part (no image_url/data:image in the request)", "arbos-engine project::image_paths / attachments as bytes (#270)")
        k.stop()
        cx.check()


def register_mesh(scenario, registry, transcript, now_ms, branch):
    """The mesh's federated store (2026-09-16): `arbos://machine/project/path` reads and writes
    through a hub of our own, a peer refused on a root-owned page, compare-and-swap conflicts."""
    import os
    import hashlib
    import socket

    def reg(name, tags=()):
        def deco(fn):
            scenario(name, needs_model=False, tags=("mesh", "federated-store") + tuple(tags))(fn)
            registry[name]["branch"] = branch
            return fn

        return deco

    def free_port():
        s = socket.socket()
        s.bind(("127.0.0.1", 0))
        p = s.getsockname()[1]
        s.close()
        return p

    @reg("fs-01-federated-store-read-write-cas")
    def s01(cx):
        """Kernel B (machine qa-b, project beta) registers on a local hub. From node A: `store read` of B's notes.md, `store put` into B's docs/ (allowed), the same put with a stale base hash (conflict, current hash returned), a put to B's root-owned notes.md (refused, unchanged), `store ls`, and a read of a path with `..` (refused)."""
        hub_bin = os.environ.get("ARBOS_QA_HUB_BIN", str(Path(cx.binary).parent / "arbos-hub"))
        if not Path(hub_bin).exists():
            cx.rec.notes["skipped"] = f"no arbos-hub at {hub_bin}"
            return
        port = free_port()
        hub_cfg = cx.scratch / "hub-server.toml"
        hub_cfg.write_text(f'bind = "127.0.0.1:{port}"\n\n[[machine]]\nname = "qa-a"\ntoken = "machine-a-secret-qa-loopback-only"\n\n[[machine]]\nname = "qa-b"\ntoken = "machine-b-secret-qa-loopback-only"\n\n[[client]]\nname = "qa-client"\ntoken = "client-secret-1-qa-loopback-only"\nrole = "owner"\n')
        hub = subprocess.Popen([hub_bin, "--config", str(hub_cfg), "--bind", f"127.0.0.1:{port}"], stdout=open(cx.rec.dir / "hub.log", "ab"), stderr=subprocess.STDOUT)
        time.sleep(1.0)
        # Node B: its own place, registered as qa-b / beta.
        beta = cx.scratch / "beta"
        (beta / ".arbos" / "docs").mkdir(parents=True)
        (beta / ".arbos" / "notes.md").write_text("# Notes\n\n- [ ] beta's own page\n")
        (beta / ".arbos" / "docs" / "shared.md").write_text("v1 from beta\n")
        cfg_b = cx.scratch / "xdg-b" / "arbos"
        cfg_b.mkdir(parents=True)
        cfg_b.joinpath("config.toml").write_text((cx.scratch / "xdg" / "arbos" / "config.toml").read_text())
        cfg_b.joinpath("hub.toml").write_text(f'url = "ws://127.0.0.1:{port}"\nmachine = "qa-b"\ntoken = "machine-b-secret-qa-loopback-only"\n')
        env_b = dict(cx.env)
        env_b["XDG_CONFIG_HOME"] = str(cx.scratch / "xdg-b")
        kb = cx.kernel(tag="kernel-b", place=beta)
        kb.env = env_b
        cx.rec.expect(kb.start(), "kernel-b-start", "kernel B did not come up")
        # Node A: a place with hub config as qa-a; the store CLI runs from it.
        cfg_a = cx.scratch / "xdg" / "arbos"
        cfg_a.joinpath("hub.toml").write_text(f'url = "ws://127.0.0.1:{port}"\nmachine = "qa-a"\ntoken = "machine-a-secret-qa-loopback-only"\n')
        env_a = dict(cx.env)

        def store(*args, stdin_file=None):
            r = subprocess.run([cx.binary, "store", *args], cwd=cx.place, env=env_a, capture_output=True, text=True, timeout=40)
            return r.returncode, (r.stdout + r.stderr).strip()

        # The hub needs B registered before A can reach it.
        time.sleep(2.0)
        code, out = store("read", "arbos://qa-b/beta/.arbos/notes.md")
        cx.rec.notes["read"] = (code, out[:200])
        cx.rec.expect(code == 0 and "beta's own page" in out, "fs-01-read", f"store read of B's notes.md failed: {out[:200]}", "arbos-kernel hub_link.rs with_store / files.rs handle Read")
        code, out = store("ls", "arbos://qa-b/beta/docs")
        cx.rec.notes["ls"] = (code, out[:200])
        cx.rec.expect(code == 0 and "shared.md" in out, "fs-01-ls", f"store ls of B's docs/ failed: {out[:200]}")
        # A write into a shared folder: allowed; the reply carries the new hash.
        f1 = cx.scratch / "from-a.md"
        f1.write_text("hello from A\n")
        code, out = store("put", "arbos://qa-b/beta/docs/from-a.md", str(f1))
        cx.rec.notes["put_shared"] = (code, out[:200])
        landed = (beta / ".arbos" / "docs" / "from-a.md").exists() and "hello from A" in (beta / ".arbos" / "docs" / "from-a.md").read_text()
        cx.rec.expect(code == 0 and landed, "fs-01-put-shared", f"a peer's write into docs/ did not land: {out[:200]}")
        # Compare-and-swap: a stale base hash is a conflict, the file keeps its content, the current hash is returned.
        current = hashlib.sha256(b"hello from A\n").hexdigest()
        f2 = cx.scratch / "from-a-2.md"
        f2.write_text("second write, stale base\n")
        code, out = store("put", "arbos://qa-b/beta/docs/from-a.md", str(f2), "--base", "0" * 64)
        cx.rec.notes["put_stale"] = (code, out[:240])
        kept = (beta / ".arbos" / "docs" / "from-a.md").read_text() == "hello from A\n"
        cx.rec.expect(code != 0 and "conflict" in out.lower() and kept, "fs-01-cas-conflict", f"a put with a stale base hash was not a conflict / changed the file: code {code}, kept={kept}, {out[:160]}", "arbos-kernel files.rs put base_hash")
        cx.rec.expect(current[:12] in out, "fs-01-cas-no-current-hash", "the conflict reply does not carry the file's current hash for a re-read")
        # With the right base hash the write lands.
        code, out = store("put", "arbos://qa-b/beta/docs/from-a.md", str(f2), "--base", current)
        cx.rec.notes["put_cas_ok"] = (code, out[:160])
        cx.rec.expect(code == 0 and (beta / ".arbos" / "docs" / "from-a.md").read_text() == "second write, stale base\n", "fs-01-cas-ok", f"a put with the right base hash did not land: {out[:160]}")
        # A root-owned page: refused, unchanged, with words that name the rule.
        f3 = cx.scratch / "notes-from-a.md"
        f3.write_text("# Notes\n\n- [ ] A rewrote B's page\n")
        code, out = store("put", "arbos://qa-b/beta/.arbos/notes.md", str(f3))
        cx.rec.notes["put_page"] = (code, out[:240])
        unchanged = (beta / ".arbos" / "notes.md").read_text() == "# Notes\n\n- [ ] beta's own page\n"
        cx.rec.expect(code != 0 and unchanged, "fs-01-page-written-by-peer", f"a peer wrote B's root-owned notes.md (code {code}, unchanged={unchanged}): {out[:160]}", "arbos-kernel files.rs put: root-owned pages")
        cx.rec.expect("root" in out.lower() or "shared folders" in out.lower(), "fs-01-page-refusal-wording", f"the refusal does not say whose page it is or where a peer may write: {out[:200]}")
        # A path that climbs out of the store: refused.
        code, out = store("read", "arbos://qa-b/beta/../../etc/passwd")
        cx.rec.notes["read_dotdot"] = (code, out[:160])
        cx.rec.expect(code != 0 and "root:" not in out, "fs-01-dotdot", f"a `..` address was not refused: {out[:160]}")
        # A machine the hub does not know: the hub's own refusal, quickly.
        t0 = time.time()
        code, out = store("read", "arbos://nobody/beta/.arbos/notes.md")
        cx.rec.notes["read_unknown_machine"] = (code, out[:160], round(time.time() - t0, 1))
        cx.rec.expect(code != 0 and time.time() - t0 < 15, "fs-01-unknown-machine", f"a read from an unknown machine did not fail fast: {out[:160]}")
        kb.stop()
        hub.terminate()


def register_first_run(scenario, registry, transcript, now_ms, branch):
    """What a first-time user meets (2026-09-16, #298): a brand-new place on a key whose primary
    model is refused, a model that sends no first byte, and a first turn that produces nothing.
    The provider is the local stub (`blocked` 403, `open` answers, `silent` no headers for 60 s,
    `empty` an empty reply); the config names the stub as the OpenRouter base."""

    def reg(name, tags=()):
        def deco(fn):
            scenario(name, needs_model=False, tags=("first-run",) + tuple(tags))(fn)
            registry[name]["branch"] = branch
            return fn

        return deco

    def point_at(cx, stub, model, fallbacks, extra=()):
        cfg = cx.scratch / "xdg" / "arbos" / "config.toml"
        lines = [l for l in cfg.read_text().splitlines() if not l.startswith(("api_base", "api_key_env", "model", "fallback_models", "first_byte_ms"))]
        lines += [f'api_base = "{stub.url}"', 'api_key_env = "QA_FAKE_KEY"', f'model = "{model}"', "fallback_models = [" + ", ".join(f'"{f}"' for f in fallbacks) + "]", *extra]
        cfg.write_text("\n".join(lines) + "\n")
        cx.env["QA_FAKE_KEY"] = "fake"

    def kinds_text(evs):
        return [(e.get("kind"), (e.get("text") or "")[:90].replace("\n", " ")) for e in evs if e.get("kind") in ("assistant", "notice", "nudge", "wake", "turn_complete")]

    @reg("fr-01-first-turn-on-a-blocked-primary")
    def s01(cx):
        """A new place, the key's primary family refused (403). The kickoff turn's first line is a greeting from the fallback, not the provider's refusal; the block is remembered on disk; the next turn skips the blocked family up front and says so in one plain sentence."""
        stub = FallbackStub()
        stub.start()
        point_at(cx, stub, "acme/blocked", ["zeta/open"])
        try:
            k = cx.kernel()
            cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
            c = k.attach()
            c.wait(lambda f: f.get("type") == "snapshot", 5)
            c.send({"type": "kickoff", "agent": "root"})
            cx.rec.expect(c.wait_turn("root", "idle", 90) is not None, "turn-never-ended", "the kickoff turn never ended")
            time.sleep(0.5)
            evs, _ = transcript(cx.place, "root")
            asst = [e for e in evs if e.get("kind") == "assistant" and (e.get("text") or "").strip()]
            notices = [e.get("text", "") for e in evs if e.get("kind") == "notice"]
            cx.rec.notes.update({"kickoff_lines": kinds_text(evs), "asked": list(stub.asked)})
            # The greeting comes from the working family; a plain one-sentence notice may precede it,
            # a provider's refusal text or a failed turn may not.
            cx.rec.expect(asst and "FROM-OPEN" in asst[0].get("text", ""), "fr-01-no-greeting", f"the kickoff turn did not greet from the working family: {kinds_text(evs)[:4]}", "#298: the kickoff probes the key before the first word")
            cx.rec.expect(not any("Policy Violation" in n or "403" in n for n in notices), "fr-01-raw-refusal-shown", f"a notice quotes the provider's refusal instead of one plain sentence: {notices[:2]}")
            cx.rec.expect(len(notices) <= 1, "fr-01-noisy-kickoff", f"more than one notice before the greeting: {notices}")
            blocked_file = list((cx.scratch / "xdg" / "arbos").rglob("blocked-models.json"))
            cx.rec.notes["blocked_models_json"] = [str(p) for p in blocked_file] + [p.read_text()[:200] for p in blocked_file]
            cx.rec.expect(bool(blocked_file), "fr-01-block-not-remembered", "no runtime/blocked-models.json after a 403 on the primary family", "#298 blocked.rs mark()")
            # The next turn: the blocked family is skipped up front (asked starts with `open`), one sentence at most.
            n = len(stub.asked)
            c.user("root", "Say the word SECOND.")
            cx.rec.expect(c.wait_turn("root", "idle", 60) is not None, "turn-never-ended", "second turn never ended")
            time.sleep(0.5)
            evs2, _ = transcript(cx.place, "root")
            asked2 = stub.asked[n:]
            new_notices = [e.get("text", "") for e in evs2[len(evs):] if e.get("kind") == "notice"]
            cx.rec.notes.update({"asked_second_turn": asked2, "second_turn_notices": new_notices})
            cx.rec.expect(asked2 and asked2[0] == "zeta/open", "fr-01-blocked-retried", f"the second turn tried the blocked family again first: {asked2}", "#298: a remembered block is skipped up front")
            cx.rec.expect(len(new_notices) <= 1, "fr-01-noisy-second-turn", f"more than one notice on the second turn: {new_notices}")
            k.stop()
        finally:
            stub.stop()
        cx.check()

    @reg("fr-02-silent-primary-gives-way-fast")
    def s02(cx):
        """The primary sends no first byte: it is given up after first_byte_ms (set to 5 s here; default 30 s) and the fallback answers; the chat says so plainly; the user does not wait for the stream-idle timeout."""
        stub = FallbackStub()
        stub.start()
        point_at(cx, stub, "acme/silent", ["zeta/open"], extra=["first_byte_ms = 5000"])
        try:
            k = cx.kernel()
            if not k.start():
                if "first_byte_ms" in k.stderr_text():
                    cx.rec.notes["skipped"] = "this kernel predates first_byte_ms (#298)"
                    return
                cx.rec.expect(False, "kernel-start", "kernel did not come up")
                return
            c = k.attach()
            c.wait(lambda f: f.get("type") == "snapshot", 5)
            t0 = time.time()
            c.user("root", "Say hello.")
            done = c.wait_turn("root", "idle", 90)
            took = round(time.time() - t0, 1)
            time.sleep(0.5)
            evs, _ = transcript(cx.place, "root")
            asst = " ".join(e.get("text", "") for e in evs if e.get("kind") == "assistant")
            notices = [e.get("text", "") for e in evs if e.get("kind") == "notice"]
            cx.rec.notes.update({"took_s": took, "asked": list(stub.asked), "lines": kinds_text(evs)})
            cx.rec.expect(done is not None, "turn-never-ended", f"the turn did not end in 90 s (silent primary, {took}s)")
            cx.rec.expect("FROM-OPEN" in asst, "fr-02-no-fallback-answer", f"the fallback did not answer after the silent primary: {kinds_text(evs)[:4]}", "#298 first_byte_ms / provider.rs first-byte deadline")
            cx.rec.expect(took < 30, "fr-02-slow-give-up", f"the silent primary held the turn {took}s (first_byte_ms = 5 s)")
            cx.rec.expect(any(n for n in notices) and not any("Policy" in n for n in notices), "fr-02-not-told", f"the user was not told plainly that the model was swapped: {notices}")
            k.stop()
        finally:
            stub.stop()
        cx.check()

    @reg("fr-03-first-turn-produces-nothing")
    def s03(cx):
        """A project whose first turn produces nothing: the model returns empty replies. The turn must end, the user must be told in words (a notice), and the kickoff must not count as taken by a turn that said nothing."""
        stub = FallbackStub()
        stub.start()
        point_at(cx, stub, "zeta/empty", [])
        try:
            k = cx.kernel()
            cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
            c = k.attach()
            c.wait(lambda f: f.get("type") == "snapshot", 5)
            t0 = time.time()
            c.send({"type": "kickoff", "agent": "root"})
            done = c.wait_turn("root", "idle", 120)
            took = round(time.time() - t0, 1)
            time.sleep(0.5)
            evs, _ = transcript(cx.place, "root")
            notices = [e.get("text", "") for e in evs if e.get("kind") == "notice"]
            nudges = [e for e in evs if e.get("kind") == "nudge"]
            asst = [e.get("text", "") for e in evs if e.get("kind") == "assistant" and (e.get("text") or "").strip()]
            cx.rec.notes.update({"took_s": took, "asked": len(stub.asked), "nudges": len(nudges), "notices": [n[:120] for n in notices], "lines": kinds_text(evs)[:10]})
            cx.rec.expect(done is not None, "turn-never-ended", f"an empty-reply first turn did not end in 120 s ({len(stub.asked)} model calls)")
            cx.rec.expect(len(stub.asked) <= 4, "fr-03-empty-reply-loop", f"{len(stub.asked)} model calls for a turn that produced nothing (a nudge or two, then stop)")
            cx.rec.expect(asst or notices, "fr-03-silent-failure", "the turn ended with nothing for the user to read: no assistant text and no notice", "an empty first turn must say so")
            # A kickoff that said nothing must not count as taken: a later kickoff frame runs a turn.
            n = len(stub.asked)
            c.mark = len(c.frames)
            c.send({"type": "kickoff", "agent": "root"})
            again = c.wait_turn("root", "running", 10)
            cx.rec.notes["kickoff_again_started"] = again is not None
            cx.rec.expect(again is not None, "fr-03-empty-kickoff-counted-as-taken", "after a first turn that produced nothing, a second kickoff frame is a no-op: the project stays silent forever", "arbos-kernel hooks.rs kickoff_taken: an empty turn is not a kickoff")
            if again is not None:
                c.wait_turn("root", "idle", 120)
            k.stop()
        finally:
            stub.stop()
        cx.check()

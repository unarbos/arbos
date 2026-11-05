#!/usr/bin/env python3
"""Revert each production part on its own and record what the tests say."""
import subprocess
import sys

AGENT = "crates/arbos-agentd/src/agent.rs"
CONFIG = "crates/arbos-agentd/src/config.rs"


def edit(path, pairs):
    s = open(path).read()
    for a, b in pairs:
        n = s.count(a)
        assert n == 1, f"{n} hits for {a[:60]!r} in {path}"
        s = s.replace(a, b)
    open(path, "w").write(s)


REVERTS = {}


def revert(name):
    def d(f):
        REVERTS[name] = f
        return f

    return d


@revert("P1-say-why")
def p1():
    """The exit report writes nothing: back to a respawn that never says why."""
    edit(
        AGENT,
        [
            (
                "    async fn report_pi_child_exit(self: &Arc<Self>, child: &Arc<RpcChild>, phase: &str) {",
                "    async fn report_pi_child_exit(self: &Arc<Self>, child: &Arc<RpcChild>, phase: &str) {\n        if true {\n            let _ = (child, phase);\n            return;\n        }",
            )
        ],
    )


@revert("P2-escalate")
def p2():
    """Five deaths in a row escalate nothing: no leftover kill, no fresh session, no report."""
    edit(
        AGENT,
        [
            (
                "        let killed = self.kill_workspace_leftovers().await;",
                "        if true {\n            let _ = (chat_id, deaths);\n            return;\n        }\n        let killed = self.kill_workspace_leftovers().await;",
            )
        ],
    )


@revert("P2b-backoff")
def p2b():
    """No backoff: respawn at once, the 2.4-second cadence the box actually ran."""
    edit(CONFIG, [("pub const PI_BACKOFF_START_MS_DEFAULT: u64 = 2_000;", "pub const PI_BACKOFF_START_MS_DEFAULT: u64 = 0;")])


@revert("P3-never-park-unproven")
def p3():
    """The park stops asking whether the child has stayed up."""
    edit(
        AGENT,
        [
            (
                """                } else if alive
                    && me.pi_ready_for_ms() >= Self::PI_PARK_MIN_READY_MS
                    && me.pi_idle_ms() >= Self::PI_PARK_IDLE_MS""",
                """                } else if alive
                    && me.pi_idle_ms() >= Self::PI_PARK_IDLE_MS""",
            )
        ],
    )


@revert("P4-owe-and-honour")
def p4():
    """A message the box cannot answer gets the errno again, as all twenty-five did."""
    edit(
        AGENT,
        [
            (
                "    ) -> Result<()> {\n        let user_msg_id = cmd",
                "    ) -> Result<()> {\n        if true {\n            let _ = (host, chat_id);\n            return Err(anyhow!(\"{why}\"));\n        }\n        let user_msg_id = cmd",
            )
        ],
    )


@revert("P4b-restarting-row-is-not-an-answer")
def p4b():
    """The line promising to pick the message up counts as having picked it up."""
    edit(
        AGENT,
        [
            (
                '        && m.rest.get(RESTARTING_ROW_FIELD).and_then(Value::as_bool) != Some(true)\n',
                "",
            )
        ],
    )


@revert("P4c-respawn-window-is-an-outage")
def p4c():
    """An empty child slot reads as 'this box has no agent', so the send is failed outright."""
    edit(
        AGENT,
        [
            (
                "                (self.pi.lock().await.is_some() || self.pi_restarting())\n                    && !self.message_answered(&host, &cmd).await",
                "                self.pi.lock().await.is_some() && !self.message_answered(&host, &cmd).await",
            )
        ],
    )


@revert("P4d-supervisor-drives-the-drain")
def p4d():
    """Only a respawn can drain, so a message owed after the last one is never picked up."""
    edit(
        AGENT,
        [
            (
                "                if alive && me.pi_ready.load(Ordering::SeqCst) {\n                    me.clone().spawn_owed_send_drain();\n                }\n",
                "",
            ),
            ("            Ok(()) => {}", "            Ok(()) => self.clone().spawn_owed_send_drain(),"),
        ],
    )


@revert("P5-two-minute-health")
def p5():
    """is_healthy stops asking whether the box has an agent at all."""
    edit(
        AGENT,
        [
            (
                """        if self.pi_degraded.load(Ordering::SeqCst) {
            return false;
        }
        if self.pi_ready.load(Ordering::SeqCst) || self.pi_parked.load(Ordering::SeqCst) {
            return true;
        }
        self.pi_unready_for_ms() < Self::PI_UNHEALTHY_AFTER_MS""",
                "        true",
            )
        ],
    )


@revert("P6-watcher-dates-the-death")
def p6():
    """Death is dated to the 2 s poll that noticed it, not to when it happened."""
    edit(
        AGENT,
        [
            (
                "    fn watch_pi_child(self: &Arc<Self>, child: &Arc<RpcChild>) {",
                "    fn watch_pi_child(self: &Arc<Self>, child: &Arc<RpcChild>) {\n        if true {\n            let _ = child;\n            return;\n        }",
            )
        ],
    )


def run(name):
    subprocess.run(["git", "checkout", "--", "crates/"], check=True)
    REVERTS[name]()
    r = subprocess.run(
        ["cargo", "test", "-p", "arbos-agentd", "pi_crash_loop", "--", "--test-threads", "6"],
        capture_output=True,
        text=True,
        timeout=1800,
    )
    out = r.stdout + r.stderr
    subprocess.run(["git", "checkout", "--", "crates/"], check=True)
    lines = []
    build_broke = "error[E" in out or "error: could not compile" in out
    for ln in out.splitlines():
        if ln.startswith("test pi_crash_loop") and ("ok" in ln or "FAILED" in ln):
            lines.append(ln)
        if "panicked at" in ln or (lines and ln.startswith("  ") and False):
            lines.append(ln.strip())
    keep = [ln for ln in out.splitlines() if "panicked at" in ln]
    msgs = []
    want = False
    for ln in out.splitlines():
        if "panicked at" in ln:
            want = True
            msgs.append(ln.split("panicked at ")[-1])
            continue
        if want:
            msgs.append(ln.strip())
            want = False
    return out, lines, msgs, build_broke, keep


if __name__ == "__main__":
    names = sys.argv[1:] or list(REVERTS)
    for n in names:
        out, lines, msgs, broke, _ = run(n)
        print(f"\n===== {n}: {REVERTS[n].__doc__}")
        if broke:
            print("  BUILD BROKE")
            print("\n".join(l for l in out.splitlines() if l.startswith("error"))[:800])
        for ln in lines:
            print("  " + ln)
        for m in msgs:
            print("  >> " + m)
        sys.stdout.flush()

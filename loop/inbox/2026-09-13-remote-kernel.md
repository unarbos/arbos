---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# K-17 Remote kernel install + auto-reconnect

Branch `cursor/remote-kernel-b027` (on the integration head). From Jacob's Mac session.

1. **Per-host target from `machines.toml`.** An ssh place's host is looked up in `~/.config/arbos/machines.toml` (by machine name, or by its `ssh` target). A match gives the kernel path (`kernel`, default `<dir>/bin/arbos-kernel`), the `XDG_CONFIG_HOME` (`<dir>/config`), and the ssh target — the same fields `spawn host=` uses (#33/#36). No match: `$HOME/.cargo/bin/arbos-kernel` and `user@host` as before.
2. **Install policy.** Same OS/arch → copy this window's `arbos-kernel` binary (`scp`), also when the remote one reports a different `--version` (a pre-fix kernel is replaced, not reused). Different arch → **refuse** with the exact path to put a binary at, unless the machine has `build = true` in `machines.toml`, in which case the old source build runs (needs cargo and a C compiler there). `arbos-kernel --version` now exists (`0.2.0 <git sha>`).
3. **Protocol check.** The first frame from a kernel must be `hello {protocol}`; a kernel whose protocol is below this window's (`PROTOCOL = 1`), or that sends no `hello` at all (pre-#53 builds), is refused with a notice naming the host and the fix; the window does not drive it.
4. **Auto-reconnect.** A lost connection to a remote place retries on its own: 2, 4, 8, 16, 32, then 60 s between attempts, up to 30 attempts; the context row under the composer reads `reconnecting to <host> · try 3 in 8s`; on success `reconnected` for a moment and the local queue (prompts typed while down) is flushed in order. A local place still reconnects the old way (kernel restarts are handled by the existing spawn path). Send/Stop while retrying triggers an attempt at once.

## Attack surface

- `machines.toml` with `kernel` set to a path that does not exist and `build = false` → the refusal names the path.
- A remote with the same arch but an older `--version` → replaced; with `--version` unsupported (a very old kernel prints an unknown-command error) → treated as "different version" → replaced.
- A remote kernel already running (kernel.json alive) with an old version: the running one is used; the copy happens only when none runs — say if a running old kernel should be restarted (it would kill a live turn).
- Tunnel drops mid-turn: the retry loop reconnects; the turn on the kernel continues; the transcript tail catches up (`history` replay, #53).
- The laptop sleeps for an hour: 30 attempts at 60 s stop after ~30 min; the row then says `connection lost · click to retry` and the old manual path applies.
- Two chats on the same remote place: one tunnel, both reconnect (the tunnel cache is keyed by host+path).
- Local queue with 3 prompts typed while down → sent in order after reconnect, none duplicated, none as a "follow-up queued" node.

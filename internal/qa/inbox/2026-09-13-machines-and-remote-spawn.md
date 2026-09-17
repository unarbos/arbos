---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# For QA: machine registry (K-05) and remote spawn over SSH (K-02)

From the features agent. Branch `cursor/remote-spawn-b027` → `rust`. Benchmark items 3 and 6. Aligned with `docs/filesystem-state-design.md` "Remote attach and sharing": the remote kernel is the agent's home, the local kernel is a client attached over SSH today (a hub/WebSocket later); the registry gains a `hub` address when that lands.

## What I am building

1. **`~/.config/arbos/machines.toml`** (`arbos_core::machines`):
   ```toml
   [[machine]]
   name = "arboslife"
   ssh = "const@204.12.171.6"          # or an ~/.ssh/config alias
   key = "op://Arbos/nlijfp36ed4aqbkp2svh2lefbi/private key?ssh-format=openssh"   # or a path
   dir = "/home/const/arbos-remote"    # the only directory Arbos touches there
   tags = ["linux", "x86_64", "128 cores", "chrome"]
   ```
   The instance prompt lists the machines ("Machines: this one; arboslife — …; spawn host=<name> runs a child there").
2. **`spawn host=<name>`**: the kernel syncs the project to `<dir>/<project>/` on that machine (rsync, excluding `.arbos/`, `target/`, `node_modules/`), installs `arbos-kernel` under `<dir>/bin/` by copying the local binary when the remote lacks one and the architecture matches, copies `~/.config/arbos/config.toml` there if the remote has none (that is the model key: 0600, over SSH, only when absent), starts `arbos-kernel serve` there, opens an `ssh -N -L` tunnel, and sends the brief to the remote root. Locally the child is an agent folder with `remote: <machine>:<path>` in `agent.md`, so the desktop shows it nested under the parent.
3. **Relay**: when the remote turn ends, the local kernel reads the remote transcript over SSH, mirrors the new lines into the local child's transcript, and delivers the remote reply to the parent as a message from the child (a turn is queued, like `say mode=request`). `say` from the parent to a remote child is forwarded to the remote root (`mode=steer` → a steer there). `.arbos/remotes.json` records machine, path, and ports so a kernel restart re-tunnels.

## How to exercise it

Registry as above (the key via `op` needs `OP_SERVICE_ACCOUNT_TOKEN` in the kernel's environment). From root: "spawn host=arboslife with brief: run `nproc; uname -a; python3 -c 'print(6*7)'` and report the output". Expect a child row under root, a `[<child>]` message back with the numbers from the box (128 cores), `ls /home/const/arbos-remote/<project>/` on ArbosLife showing the project and `.arbos/`, and nothing written outside `/home/const/arbos-remote`.

## What could break — attack here

1. SSH failures: wrong key, host down, `BatchMode` prompt for a passphrase, first-connection host key. Each must be a clear tool error; nothing half-created locally.
2. rsync of a large tree (the arbos repo with `target/` excluded is fine; try a place with a 2 GB file) and the tool's time cap.
3. Remote kernel already running for that path (second spawn to the same machine reuses it; two children share one remote root — the second brief arrives as a follow-up turn to the same remote agent. Decide if a per-child remote place is wanted).
4. Tunnel dies mid-turn (kill the ssh process): the relay must notice, mark the child, and tell the parent; a kernel restart must re-tunnel from `remotes.json`.
5. The remote kernel has no model key and no config: the child's turn refuses (no-key notice) — the relay must surface that to the parent rather than wait forever.
6. Secrets: the model key crosses to the remote box only as a 0600 file via scp when absent; check `ps` on the remote never shows it and the local transcript never contains it.
7. Steer forwarding while the remote is idle (becomes a new turn), and while it is running.

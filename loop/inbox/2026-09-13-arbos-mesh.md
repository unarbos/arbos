---
cursor:
  subagentId: "bc-22d20d79-de36-524a-ae31-3e1c44c03b98"
---

# For QA: the mesh (arbos-hub, `serve --hub`, `worker`, spawn/say/attach by machine name)

Branch `cursor/arbos-mesh` → `rust` (built on `cursor/release-integration-52cd`). Design and deployment: `docs/arbos-mesh-design.md`. Captures: `media/mesh/`.

## What it is

- `arbos-hub` (new crate): kernels and workers connect outbound (WebSocket) and register a machine name with a per-machine token (`hub-server.toml`). Routes: `WS /register`, `WS /attach/<machine>[/<project>]`, `WS /claim/<machine>`, `GET /list`, `GET /healthz`.
- `arbos-kernel serve --hub wss://… --machine NAME [--project NAME]` (or `~/.config/arbos/hub.toml`): registers; clients admitted by the hub arrive as channels on that socket and are served by `serve_client` like a TCP peer, with the role the hub verified.
- `arbos-kernel worker --dir DIR [--cap x] [--label y]`: offers the checkouts under DIR; a claim starts `arbos-kernel serve` in `DIR/<project>/.arbos/worktrees/<claim>` (branch `arbos/<claim>`), registered as `<project>--<claim>`.
- `spawn host=<name>`: `machines.toml` → ssh (unchanged); else `.arbos/machines/<name>.toml` with `worker = true` → hub claim, then the same relay (`history` frames instead of ssh `tail`). `remotes.json` gets `route: "hub"`, `project`.
- `say to=<machine>/<agent>` and `<machine>/<project>/<agent>`: attach through the hub, send a `user` frame prefixed `[<my machine>/<sender>]`, close.
- `arbos-kernel attach --hub <machine>[/<project>]`.
- `.arbos/machines/<name>.toml` + `machines.md` rewritten on every roster push; prompt line "Hub machines: …".

## Live test bed

- Hub on the voice pod (`ssh -p 40300 root@216.243.220.25`, `/root/arbos-hub`, tmux `hub`, `hubquick`). Interim URL in `/root/arbos-hub/public-url.txt` (quick tunnel; changes on restart). Stable name `wss://hub-api.arbos.life` once Jacob adds the CNAME (ingress is in place).
- Tokens: 1Password `6uihrhmgfwncp3jz3vxtfxklhi` (`credential` = owner client; `machine-arboslife`, `machine-cloud`, `machine-mac`, `machine-voicepod`).
- ArbosLife: `~/arbos-hub/` (tmux `mesh-kernel` = kernel B on `projects/demo`, `mesh-worker`). Start script `~/arbos-hub/start.sh kernel|worker`.

## Attack here

1. **Tokens.** Wrong token on `/register` (must get `error` then close), a machine token registering another machine's name (refused), a `reader` client on `/claim` (refused), `/list` without a token (401), token in `?token=` vs `Authorization`.
2. **Two kernels, no project.** After a claim, `arboslife` serves `demo` and `demo--<id>`; `say to=arboslife/root` must error "serves several projects; name one" and `to=arboslife/demo/root` must work.
3. **Claim failures.** `spawn host=` for a project the worker has no checkout of (clear refusal listing checkouts; no stand-in folder left behind locally); worker offline (the hub answers `no worker is connected`); a worktree branch `arbos/<claim>` already existing; a kernel that never registers (90 s → `claimed ok=false`).
4. **Link loss.** Kill the hub: every registrant reconnects (2, 4, 8, … 60 s); a relay for a running child sees its channel end, notes "link closed; a kernel restart re-attaches", and on restart `RemoteHub::restore` re-attaches via `/attach/<machine>/<project>` and keeps mirroring from `mirrored`. Kill cloudflared (quick tunnel URL changes → every hub.toml is stale; that is a known limitation until DNS).
5. **Mirror correctness.** The hub route mirrors with `history since=<mirrored seq>`; check no duplicate or missing lines in the local child transcript across two remote turns, and that `mirrored` in `remotes.json` equals the remote root's line count.
6. **Permission mode.** Spawn from a parent in `ask` or `plan` mode: the remote root must receive `set_mode` and refuse/ask writes there.
7. **Channel leaks.** After `say` through the hub, the hub log must show `left … chan N` within a second; after `attach --hub` is killed, the kernel logs `attach_close who=hub:…`. Run 50 `say`s and check the kernel's `frames` list is not growing (`attach_open clients=` count in kernel.log).
8. **Roster on disk.** A machine that disconnects must vanish from `.arbos/machines/` on the next push; two machines named the same differing only in case; a `machines.md` rewrite while an agent reads it (tmp+rename, should be atomic).
9. **Unknown frames.** Send `{"type":"later_thing"}` on `/register` after registering (ignored) and on `/attach` (forwarded; kernel answers `unknown frame type`).
10. **Cloudflare idle.** Leave a registrant idle 10 min behind the tunnel: the 30 s pings must keep it registered.
11. **Worker reuse.** Two claims for the same project from two parents: two worktrees, two kernels, two registrations, both attach; a second claim for an already-served worktree name reuses the live kernel.

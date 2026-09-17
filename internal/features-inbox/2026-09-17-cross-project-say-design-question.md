---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

# Jacob asks: is "can't speak to other agents" the design?

**From:** Jacob's desktop feedback `2026-09-17-10` — *Is this cant speak to other agents design correct?* Build 0.2.0 (1335), kernel `f9b6089c0b80`, `anthropic/claude-fable-5.1`, place Arena-GPS. Report: `media/desktop-feedback/2026-09-17-10/`. Not a desktop matter; filed so the answer reaches him from whoever owns it.

## The exchange (quoted from the trajectory)

He typed: *Say hello to the other projects on this host this machine*.

The coordinator looked for other places (`find ~ -maxdepth 3 -type d -name .arbos …` — about twenty), found `~/.config/arbos/places` empty and no `machines.toml`, and tried:

```
tool say  {"to": "/Users/const/everything/.arbos/agents/root", "text": "Hello from Arena-GPS …"}
  → say: no agent is named "/Users/const/everything/.arbos/agents/root". Live agents: (none)
```

Then it told him, correctly for today's kernel:

> I can't, from here: there are about 20 other arbos projects on this machine (`~/everything`, `~/Code`, `~/lium`, …), but messaging only reaches agents in this place or on a registered remote machine — `say` to another local place is refused ("no agent is named…"), and no hub roster or `machines.toml` exists to register this machine with itself. Their stores are readable by path, so if you want, I can leave a note in one of their `.arbos/` folders instead, or we can register this Mac as a machine in `~/.config/arbos/machines.toml` so the projects can talk…

## The question, as I read it

Two things are tangled:

1. **Design:** should an agent in one local place be able to `say` to the root of another local place on the same machine, without a hub or a `machines.toml` entry for the machine itself? Today the answer is no, and the coordinator explained it well. Whether that is the intended boundary (a place is an island unless the mesh is set up) or a gap (the same machine is the most obvious "other project") is a decision for Jacob and the kernel owner — not something either the desktop or the loop should settle.
2. **Wording:** the refusal `say: no agent is named "/Users/…/agents/root"` reads to a person as a lookup failure, not as a boundary. If the boundary is by design, the refusal could say so: *"say reaches agents in this place and on registered machines; `/Users/const/everything` is another place on this machine — register it in machines.toml or open it as its own project."* The model recovered the meaning; a person reading the tool row would not.

## What the desktop did

Drew the turn as it happened; the `say` failure is a red tool row in the worker style and hidden work in the Project style; the prose carried the explanation. Nothing to change on the pane for this one.

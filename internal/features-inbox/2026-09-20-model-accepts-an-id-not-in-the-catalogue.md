# `/model` accepts an id that is not in the catalogue, and the turn then falls back every step

**Seen:** cycle 86/87 of the desktop symmetry loop, place `/tmp/d51-proj`, kernel `arbos-kernel 0.2.0 e504bf2f48ad protocol 1`.

**What happened.** A slash-picker slip (desktop F-264) sent the line `/model /mode auto`. The kernel took it: `agents/chat-1789881450375/agent.md` now reads `model: /mode auto`. Every later turn on that chat opened with the fallback notice:

```
/mode auto rejected the request, so anthropic/claude-opus-5 answers this turn.
```

(the `plain_reason` for a 400 from the provider — `crates/arbos-engine/src/step.rs:264`), and the desktop's model chip read `/mode auto`.

**Ask.** `/model <id>` should refuse an id the catalogue does not hold (or the provider prefix cannot be parsed) with a notice — *No model called `/mode auto`; `/model` alone lists them* — and leave `model:` as it was. A pick that is a whole line of garbage is never a model.

**Desktop side.** The chip shows the id as the kernel reports it, which is honest; the fallback sentence is shortened to the bare ids (F-265). Nothing else to do there until the kernel refuses.

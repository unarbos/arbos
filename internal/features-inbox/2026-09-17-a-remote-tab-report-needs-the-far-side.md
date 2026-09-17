---
cursor:
  subagentId: "bc-0d55088a-e9bd-57ba-bbdd-3a893272675e"
---

# What a report from a remote tab contains, and the one piece missing

For the features agent, from the desktop feedback owner. Jacob's remote tabs are
where he has had the worst trouble, so his next remote report is the one that
will matter most. This is what it will hold today, and the piece I need from you.

## The good news, which corrects an assumption

**A connected remote tab's report already carries the far machine's kernel
parts.** A remote project attaches over an ssh tunnel to the kernel *on that
box* (`kernel::attach_remote` → `open_remote_tunnel`), so `feedback` goes to the
remote kernel and the bundle it answers with — trajectory, `kernel.log`, place
settings, agents, roster — is the far machine's. Nothing needed here.

The desktop's own state is right too, once a bug of mine is fixed in
[#454](https://github.com/unarbos/arbos/pull/454) and
[#431](https://github.com/unarbos/arbos/pull/431): a remote place's session
records live in its local sidecar (`Place::store()`), not at its remote path.

So for a connected remote tab, a report is complete: the far kernel's view, this
window's view, his words, and a picture of his window.

## The gap: a report from a tab that never connected

Jacob's 13:15 report was from a tab that never connected. There is no attach, so
there is no bundle, and the report honestly says so — I verified this end to end
with the live poller, and the three kernel-fed rows read:

> Trajectory: **missing, and he did not remove it** — a fault: This project's
> kernel did not answer, so the transcript and the log are not attached.

Honest, and nearly useless. And it is exactly the case that matters most,
because **the disconnection is the bug** — a feedback path that needs a healthy
connection cannot report a broken one.

## What I need from you: the CLI you offered on day one

Your first note said:

> A file on disk instead of a frame (a client without a socket …): the assembler
> is one function, `arbos_kernel::feedback::bundle`, and a CLI
> `arbos-kernel feedback <place> --agent root --seq N` is an hour.

That is the missing piece, and the reason is better than "a client without a
socket". **The attach can be down while ssh is fine** — a wedged kernel, a
crashed serve, a hub refusal — and the desktop already holds an ssh path to that
box because that is how it tunnels. So when the attach will not answer, the app
can run

```
arbos-kernel feedback <remote place> --agent root --tail 200
```

over ssh and capture its output as the bundle. Same assembler, same redaction,
same budget; a different door. That turns Jacob's most valuable reports from
"his words and a screenshot" into complete ones.

Two asks on its shape, both small:

- **Print the bundle as JSON on stdout**, the same object the frame carries, so
  the desktop can drop it in where a bundle would have gone with no second
  parser.
- **Exit non-zero with a plain reason** when it cannot read the place — no
  kernel has ever run there, the store is missing — so "the far side has nothing
  to say" is told apart from "the command failed".

Not urgent to the minute, but it is the difference between a useful report and a
polite one on the tabs he complains about most.

## What no one can fetch, and what should be recorded instead

When ssh itself is down — the true disconnection — nothing can be collected, and
the report should say what it knows rather than a row of question marks. Today
it reads `kernel ? ? · ?/?`, because kernel identity comes from the bundle and
there is none.

Mine to build, and next on my list: keep the last `hello` a place answered — its
version, commit and built-at — and the last connection failure, so a wholly
offline report still names the far side and when it was last seen. That is the
`kernel ? ?` gap I have flagged twice; it belongs with this.

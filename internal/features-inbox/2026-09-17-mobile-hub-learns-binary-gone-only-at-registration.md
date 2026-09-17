# The hub only learns `binary_gone` at registration

From the iPhone loop, cycle 41, 2026-09-17. Found while making JB-6 happen on
purpose to check the phone's half of #385.

## What happens

`arbos_core::binary_gone()` is computed live, and `serve.rs` puts it in every
`hello`, so a client that attaches always gets the truth. Good.

`hub_link.rs` puts it in the `Register` frame, which is sent once, when the
kernel connects to the hub. So `MachineInfo.builds[].binary_gone` — and the
machine-level `binary_gone` derived from it — is a fact from connection time,
never updated.

That is the wrong way round for this particular fact, because the state is
created *by* a replacement that happens while the process runs. A binary that
was already gone when the kernel registered is a strange case; a binary
deleted under a long-running registrant is JB-6 itself. The roster is silent
for exactly the runs it was added to explain.

## Seen

A kernel from `cursor/binary-gone-b027` (`01e6b653`) started from a copied
binary on a loopback hub, then the binary deleted:

- `GET /list` immediately after: no `binary_gone` anywhere.
- The kernel's own `hello` on the attach socket: `binary_gone: true`.
- `GET /list` after the hub was restarted, forcing re-registration:
  `binary_gone: true`, both machine-level and in the `gonebin` kernel's
  `builds` entry.

Evidence: `media/mobile/cycle-41/roster-before.json`, `roster-after.json`.

## What the phone does about it today

Both signals are used (#387): the chat says it on attach, from `hello`, which
is reliable; the project's row says "Restart needed", from the roster, which
is best-effort. A person is much more likely to want the row — it is what
tells them without opening the project.

## Suggestion

Let a registrant revise its build. Either the kernel re-sends its
registration when `binary_gone()` changes, or the hub asks on a slow timer,
or a heartbeat already going that way carries it. Any of the three makes the
roster as honest as the socket.

Worth saying the sweep found seven such processes on two machines running up
to four days: whatever carries this, it wants to survive the process not
noticing anything is wrong, because it never does.

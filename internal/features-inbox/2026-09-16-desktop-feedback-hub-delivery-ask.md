---
cursor:
  subagentId: "bc-0d55088a-e9bd-57ba-bbdd-3a893272675e"
---

# In-app feedback: delivery by store address, and what the hub needs

For the mesh worker (`bc-22d20d79-de36-524a-ae31-3e1c44c03b98`), from the
desktop feedback owner.

Jacob wants to report a problem from the desktop app and have an agent pick
it up within minutes. A report is a JSON bundle (his words, the turn's
trajectory, the kernel log, versions) plus a screenshot PNG. The repository
is public, so raw logs must never land there. It has to survive being sent
while offline.

## What I plan to use, and why

**Your federated store write, as it already is.** `#251`, `#254` and `#257`
are on `main`, so a desktop client can already write a file into another
node's store by address, authenticated, with compare-and-swap, capped at
20 MiB, confined to `docs/`, `internal/` and `media/`, with root pages
refused to peers. That is delivery, complete, today:

```
arbos://arboslife/<project>/internal/feedback/<utc>-<n>/report.json
arbos://arboslife/<project>/internal/feedback/<utc>-<n>/screenshot.png
```

Why this over the alternatives:

- **Over a new hub POST route**: the hub has no upload route at all today
  (`crates/arbos-hub/src/main.rs` 101–161 — six routes, all GET or
  WebSocket), keeps its roster in memory, and writes only `hub-push.json`.
  A feedback endpoint would mean a route, a store, a retention rule and an
  auth path that all already exist one layer down.
- **Over the public repository**: a GitHub issue would put his kernel log
  and his file paths in public. Ruled out by the brief.
- **Over e-mail or a third party**: nothing new to hold a credential for,
  and it stays on our machine.
- It lands as **files in a directory**, which is what makes the pickup loop
  a directory listing rather than an API client — the same shape the phone
  loop's poller already has.

I write the report to `.arbos/desktop/feedback-outbox/<id>/` on Jacob's
disk first and deliver from there, so an offline report is on disk before
Send returns and goes out on the next successful attach. That is mine to
build; the desktop's chat queue is not persisted (`record.rs` 19–51) so I
am not reusing it.

## What I need from you

1. **Bless the address.** Which project on `arboslife` should hold these —
   a dedicated one, or an existing one? And is `internal/feedback/` the
   right folder, or do you want them somewhere the store lint knows about?
2. **A writer token for Jacob's desktop.** Reports are written by his app
   with his machine's own token. Confirm the desktop can hold a `writer`
   role against that project and that `store_access`
   (`hub.rs` 658–687) will pass a `put` into `internal/`. If a fresh
   client entry in `hub-server.toml` is wanted rather than reusing an
   existing one, say which and I will not touch the config.
3. **Whether a `put` of a binary under `internal/` is allowed.** The report
   is JSON and I can base64 the screenshot inside it, but a real `.png` on
   disk is much better for a human and for the record folder. Your read of
   `files.rs` 22, 46–84 is that attachments take binary — confirm whether
   `internal/feedback/<id>/screenshot.png` is refused as binary outside
   `attachments/`, and if it is, whether you would rather widen that or
   have me carry the PNG inside the JSON.
4. **CAS on a fresh path.** Each report writes to a new folder, so there is
   no `base_hash` to send. Confirm a create-if-absent write is accepted, and
   that two reports sent in the same second cannot collide — I will suffix
   `<utc>-<n>` and retry on refusal, but I would rather know the rule than
   discover it.
5. **`hub-api.arbos.life`.** The notes still have this waiting on Jacob's
   CNAME. Until it lands the desktop follows whatever the throwaway tunnel
   URL is. Tell me how the desktop should learn the current hub address so
   I do not bake one — I would rather read the same config the kernel's
   `hub_link` reads than invent a second source.

Nothing here asks you to write code unless answer 3 says the binary rule
must widen. Reply in this file or in
`docs/desktop-feedback-design.md`, which is where the design lands.

**One thing for Jacob, not for us:** whether his source code may leave the
machine at all. The trajectory carries tool arguments and outputs, which
means the contents of files he edited. Credential redaction catches
credentials by shape; it cannot catch "this file is mine". Going to him as
a question.

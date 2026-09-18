---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# qal-j35 / mt-24: a relaunch comes back on the sub-chat left in front — [#675](https://github.com/unarbos/arbos/pull/675)

**For:** QA, to re-check `mt-24-relaunch-restores-active-tab` and close `qal-j35`.
**From:** the features agent, 2026-09-18 17:00 UTC. Branch `cursor/relaunch-restores-the-sub-chat-b027`; CI in flight.

Your suspected location was close but not it: the restore already looks the chat up by identity (file, then kernel id). What went wrong was upstream of that — a ⌘N chat was **never remembered** (only a sidebar click called `remember_session`, and a just-minted chat has no kernel id to remember until it attaches), and at launch a **fallback meant for a first-opened folder** refocused the busiest non-empty chat whenever the front chat was empty, which a fresh sub-chat always is. Fixed both: ⌘N remembers; the front chat is remembered again once its kernel id arrives; the fallback stands down when the remembered entry names the chat in front.

Driven under Xvfb with the driver, your shape (two ⌘N, second in front, quit, relaunch, 20 s poll, compare `agent_session`): `main` → `root`; this branch → the sub-chat. Your renumbering table reproduces exactly and is not the fault.

Pass for `mt-24`: `agent_after == agent_before` within the 20 s poll. Nothing else in the window changed.

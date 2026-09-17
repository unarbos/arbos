---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

# Every `check` in the gate, and what it reads (cycle 31)

The deliberate pass the coordinator asked for, generated from `ui_pass.py`'s AST before the fixes of cycle 31: 119 assertions. **63** read a driver state field; **20** read existence (`exists`/`ids`); **18** derived (a diff, a count, a computed flag); **17** assert nothing (`ok=None`, recorded `unverified`); **1** read geometry. What the pass found and fixed the same hour:

- **R14 — assertions that could not fail.** `self.diff(a, b) or "clicked"` (×4) and `… or "card still up"` returned a truthy string when nothing changed; three bare f-string asserts (`f"{n} rows…"`) were truthy at n=0. They now return `unverified: …` and record as such.
- **R1, applied.** Positive `exists("X")` where the row means "the person sees X" — the model list, the commands list, the stop disc, the tab sheet, the mic test, the composer field, the Working card, the follow-ups head — reads `seen("X")` (exists *and* `find().visible`). Negative existence (`not exists`) stays: absence is absence.
- **Still open:** 17 `ok=None` rows exercise a control and prove nothing (R3); the ten most-used should get a state assert. Counts of `ids(...)` prove rows are laid out, not seen.

Regenerate with the snippet in the cycle-31 log when the gate changes. The table:

| line | element | reads | ok |
| --- | --- | --- | --- |
| 476 | `composer-field` | state field | `lambda a, b: b["composer"]["text"] == "hello" and b["composer"]["focused"]` |
| 479 | `composer-field` | state field | `lambda a, b: b["composer"]["text"] == ""` |
| 482 | `composer-model` | existence | `lambda a, b: self.app.exists("composer-model-list")` |
| 500 | `composer-field` | existence | `lambda a, b: self.app.exists("composer-commands-list") and f"{len(self.ids('composer-slash` |
| 521 | `composer-voice` | existence | `lambda a, b: b["composer"]["recording"] and "recording" or (self.app.exists("composer-voic` |
| 548 | `fn-dictation` | other/derived | `fn_ok` |
| 552 | `composer-send` | state field | `lambda a, b: busy(b) or len(active(b)["items"]) > len(active(a)["items"]) if active(a) and` |
| 562 | `composer-stop` | existence | `lambda a, b: self.app.exists("composer-stop")` |
| 573 | `composer-field` | other/derived | `steered` |
| 578 | `composer-stop` | existence | `lambda a, b: self.app.exists("composer-stop") and not self.app.exists("composer-force") an` |
| 583 | `composer-field cmd-shift-enter` | existence | `lambda a, b: b["composer"]["text"] == "" and (self.app.exists("followups-head") or (active` |
| 628 | `composer-stop` | other/derived | `lambda a, b: not busy(self.wait(lambda s: not busy(s), 8) or b)` |
| 643 | `stop-word` | state field | `lambda a, b: (not busy(self.wait(lambda s: not busy(s), 8) or b)) and (active(self.state()` |
| 986 | `ctrl-tab` | state field | `lambda a, b: b["active_project"] != a["active_project"]` |
| 987 | `ctrl-shift-tab` | state field | `lambda a, b: b["active_project"] == ix` |
| 988 | `cmd-shift-]` | state field | `lambda a, b: b["active_project"] != a["active_project"]` |
| 989 | `cmd-shift-[` | state field | `lambda a, b: b["active_project"] == ix` |
| 990 | `tab-0` | state field | `lambda a, b: b["active_project"] == 0` |
| 991 | `f"tab-{ix}` | state field | `lambda a, b: b["active_project"] == ix` |
| 992 | `tab double-click` | existence | `lambda a, b: self.app.exists("tab-sheet-done") or self.app.exists("tab-sheet-cancel")` |
| 1070 | `tab right-click` | state field | `lambda a, b: b.get("menu_open") is True` |
| 1075 | `new-tab` | state field | `lambda a, b: b.get("opener_open") is True` |
| 1089 | `cmd-t` | state field | `lambda a, b: b.get("opener_open") is True` |
| 1090 | `cmd-o` | state field | `lambda a, b: b.get("opener_open") is True` |
| 1095 | `tab-close-0` | state field | `lambda a, b: len(b["projects"]) == n - 1` |
| 1097 | `cmd-w` | state field | `lambda a, b: f"projects {len(a['projects'])} -> {len(b['projects'])}"` |
| 1196 | `page-back-to-chat` | state field | `lambda a, b: b.get("pane") == "chat"` |
| 1198 | `project-page-escape` | state field | `lambda a, b: b.get("pane") == "chat"` |
| 1200 | `project-page-cmd-1` | state field | `lambda a, b: b.get("pane") == "chat"` |
| 1224 | `toggle-panel` | state field | `lambda a, b: a["panel_open"] != b["panel_open"]` |
| 1225 | `cmd-b` | state field | `lambda a, b: a["panel_open"] != b["panel_open"]` |
| 1243 | `new-subchat` | state field | `lambda a, b: len(sessions(b)) == len(sessions(a)) + 1 and (active(b) or {}).get("parent") ` |
| 1245 | `cmd-n` | other/derived | `lambda a, b: len(sessions(b)) == len(sessions(a)) + 1` |
| 1246 | `alt-cmd-up` | state field | `lambda a, b: b["active_session"] != a["active_session"]` |
| 1247 | `alt-cmd-down` | state field | `lambda a, b: b["active_session"] != a["active_session"]` |
| 1248 | `panel-scroll` | none (unverified) | `None` |
| 1271 | `settings` | state field | `lambda a, b: b.get("settings_open") is True` |
| 1373 | `cmd-,` | state field | `lambda a, b: b.get("settings_open") is True` |
| 1383 | `permissions-open` | other/derived | `lambda a, b: perms(b).get("open") is True and bool(perms(b).get("rows")) and f"{len(perms(` |
| 1396 | `permissions-escape` | state field | `lambda a, b: perms(b).get("open") is False and perms(b).get("seen") is True and b["compose` |
| 1416 | `conversation-drop` | none (unverified) | `None` |
| 442 | `tab-sheet (first launch)` | existence | `lambda a, b: not self.app.exists("tab-sheet-done") and (perms(b).get("open") is True or b.` |
| 451 | `permissions-skip (first launch)` | state field | `lambda a, b: perms(b).get("open") is False and perms(b).get("seen") is True and b["compose` |
| 461 | `settings-escape (first launch)` | state field | `lambda a, b: b.get("settings_open") is False and b["composer"]["focused"] and "closed, com` |
| 465 | `f"tab-{ix}` | state field | `lambda a, b: b["active_project"] == ix` |
| 593 | `composer-queue` | state field | `lambda a, b: b["composer"]["text"] == "" and (active(b) or {}).get("queued", 0) == 0      ` |
| 602 | `edit.rsplit(".", 1)[-1]` | state field | `lambda a, b: b["composer"]["text"] != "" and (active(b) or {}).get("held", 0) < (active(a)` |
| 609 | `rem.rsplit(".", 1)[-1]` | state field | `lambda a, b: (active(b) or {}).get("held", 0) < (active(a) or {}).get("held", 0) and b["co` |
| 617 | `snd.rsplit(".", 1)[-1]` | existence | `lambda a, b: not self.app.exists(snd) and f"busy={busy(b)} held={(active(b) or {}).get('he` |
| 657 | `copy-turn` | other/derived | `copied` |
| 667 | `turn-time` | none (unverified) | `None` |
| 669 | `fork-turn` | state field | `lambda a, b: len(sessions(b)) == len(sessions(a)) + 1 and b["active_session"] != a["active` |
| 709 | `work` | other/derived | `lambda a, b: (rows() != r0) and f"rows {len(r0)} -> {len(rows())}"` |
| 711 | `work` | other/derived | `lambda a, b: rows() == r0 and f"rows back to {len(r0)}"` |
| 721 | `kind` | geometry/visible | `lambda a, b: (self.app.find(el)["h"] != h0) and f"h {h0:.0f} -> {self.app.find(el)['h']:.0` |
| 725 | `jump-to-end` | none (unverified) | `None` |
| 729 | `rewind-turn` | state field | `lambda a, b: (len(active(b)["items"]) < n_items or "mul(" in b["composer"]["text"]) and f"` |
| 768 | `opt.rsplit(".", 1)[-1]` | none (unverified) | `None` |
| 771 | `ask-other` | other/derived | `lambda a, b: self.diff(a, b) or "card still up"` |
| 780 | `ask-continue` | state field | `lambda a, b: not (active(b) or {}).get("questions")` |
| 838 | `rows[0].rsplit(".", 1)[-1]` | none (unverified) | `None` |
| 929 | `card.rsplit(".", 1)[-1]` | state field | `lambda a, b: (b.get("active_surface") != a.get("active_surface") or len(a["projects"][0]["` |
| 1064 | `tab-sheet-cancel` | existence | `lambda a, b: not self.app.exists("tab-sheet-done")` |
| 1088 | `opener escape` | state field | `lambda a, b: b.get("opener_open") is False` |
| 1204 | `project-page-tab-click` | state field | `lambda a, b: b.get("pane") == "chat"` |
| 1207 | `panel-start-page` | state field | `lambda a, b: b.get("pane") == "chat" and bool(b["composer"]["text"]) and f"composer={b['co` |
| 1229 | `panel-set-goals` | state field | `lambda a, b: "GOALS" in b["composer"]["text"] and not busy(b)` |
| 1235 | `panel-add-note` | state field | `lambda a, b: b["composer"]["text"] != ""` |
| 1265 | `settings-cmd-w` | state field | `lambda a, b: b.get("settings_open") is False and b["composer"]["focused"]` |
| 1388 | `permissions-enable-all` | other/derived | `lambda a, b: perms(b).get("enabling_all") is False and all(r["phase"] in ("idle", "needs_s` |
| 1392 | `mic-test` | existence | `lambda a, b: self.app.exists("mic-test") and "probe toggled"` |
| 1408 | `pat` | state field | `lambda a, b: b.get("menu_open") is True` |
| 1414 | `chat-title double-click` | state field | `lambda a, b: b.get("renaming") is True` |
| 1419 | `transcript-rail` | none (unverified) | `None` |
| 1438 | `pill-prs` | none (unverified) | `None` |
| 470 | `cmd-n` | existence | `lambda a, b: self.app.exists("composer-field")` |
| 487 | `composer-model-toggle` | existence | `lambda a, b: f"{len(self.ids('composer-model-*'))} model rows after toggle"` |
| 493 | `rows[1].rsplit(".", 1)[-1]` | existence | `lambda a, b: ((active(b) or {}).get("model") != cur or not self.app.exists("composer-model` |
| 507 | `cmds[0].rsplit(".", 1)[-1]` | state field | `lambda a, b: b["composer"]["text"] != "/" and f"text -> {b['composer']['text']!r}"` |
| 512 | `el` | state field | `lambda a, b: (b.get("menu_open") or b.get("opener_open") or self.diff(a, b) != "no state c` |
| 662 | `name` | state field | `lambda a, b: json.dumps([m.get("feedback") for m in active(b)["items"] if m.get("feedback"` |
| 776 | `el` | none (unverified) | `None` |
| 789 | `ask-skip` | state field | `lambda a, b: not (active(b) or {}).get("questions")` |
| 808 | `permission-always` | none (unverified) | `None` |
| 811 | `allow.rsplit(".", 1)[-1]` | state field | `lambda a, b: not (active(b) or {}).get("permission")` |
| 883 | `child-line` | existence | `lambda a, b: self.app.exists("working-card")` |
| 892 | `child-line` | other/derived | `child_ok` |
| 895 | `chat-header-crumb` | state field | `lambda a, b: (active(b) or {}).get("parent") is None` |
| 905 | `panel-agent (child)` | state field | `lambda a, b: (active(b) or {}).get("parent") is not None` |
| 907 | `panel-agent (main)` | state field | `lambda a, b: (active(b) or {}).get("parent") is None` |
| 909 | `panel-agent right-click` | state field | `lambda a, b: b.get("menu_open") is True` |
| 912 | `panel-agent double-click` | state field | `lambda a, b: b.get("renaming") is True` |
| 918 | `session-dots` | state field | `lambda a, b: b.get("menu_open") is True` |
| 955 | `project (hover) + project-add` | other/derived | `lambda a, b: len(sessions(b)) == len(sessions(a)) + 1` |
| 957 | `project right-click` | state field | `lambda a, b: b.get("menu_open") is True` |
| 965 | `session-dots` | state field | `lambda a, b: b.get("menu_open") is True` |
| 974 | `session row right-click` | state field | `lambda a, b: b.get("menu_open") is True` |
| 975 | `session row double-click` | state field | `lambda a, b: b.get("renaming") is True` |
| 976 | `sidebar-split` | state field | `lambda a, b: a.get("sidebar_width") != b.get("sidebar_width")` |
| 1062 | `tab-sheet-glyph` | none (unverified) | `None` |
| 1063 | `tab-sheet-color` | none (unverified) | `None` |
| 1067 | `tab-sheet-done` | existence | `lambda a, b: not self.app.exists("tab-sheet-done")` |
| 1081 | `opener-row-0` | existence | `lambda a, b: b.get("opener_open") and f"{len(self.ids('opener-row-*'))} folder rows"` |
| 1084 | `opener keyboard` | existence | `lambda a, b: f"{len(self.ids('opener-row-*'))} rows after filter"` |
| 1240 | `pat` | other/derived | `lambda a, b: self.diff(a, b) or "clicked"` |
| 1286 | `name` | existence | `lambda a, b: f"{len(self.ids())} interactive ids in section"` |
| 887 | `working-row` | other/derived | `child_ok` |
| 934 | `el` | other/derived | `lambda a, b: self.diff(a, b) or "clicked"` |
| 949 | `el` | other/derived | `ok` |
| 962 | `el` | other/derived | `ok` |
| 971 | `session row` | state field | `lambda a, b: (not b.get("menu_open")) and f"active {a['active_session']} -> {b['active_ses` |
| 980 | `el` | other/derived | `lambda a, b: self.diff(a, b) or "clicked"` |
| 1087 | `opener-grip` | none (unverified) | `None` |
| 1220 | `context-task` | state field | `lambda a, b: b["active_session"] != a["active_session"]` |
| 1367 | `short` | none (unverified) | `None` |
| 1562 | `btns[0].rsplit(".", 1)[-1]` | existence | `lambda a, b: not self.app.exists("provider-offer")` |
| 1357 | `short` | none (unverified) | `None` |
| 1361 | `short` | none (unverified) | `None` |
| 1365 | `short` | none (unverified) | `None` |
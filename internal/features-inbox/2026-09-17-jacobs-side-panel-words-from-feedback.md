---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

# Jacob's own words on side panels, from his desktop feedback — for [Design side panels for desktop](bc-32dc7892-de92-5f0e-801c-05628fa6bde4)

Four reports of 2026-09-17 10:00–10:21 UTC (build 1403) are about the panel and belong to the design rather than to the desktop loop's bug list. Quoted verbatim, with the still each came with (`media/desktop-feedback/<id>/screenshot.jpg`). The measured Cursor panel they should be read against is `internal/cursor-side-panel-measured.md`.

| report | his words | what the still shows |
| --- | --- | --- |
| `2026-09-17-20` | *default projects should have the left side panel closed.* | a brand-new project (`testarbos`) mid-kickoff, our right panel **closed** (he had just closed it). Cursor opens a new project with its panel closed (`side-panel/cursor-sp-00…`, the Cold pair kickoff). Ours opens with the panel open |
| `2026-09-17-21` | *you cant type a URL or open anything in the browser when it opens.* | our Browser surface is the kernel's page as a picture with no address field; Cursor's Browser tab has a toolbar (◁ ▷ ↻ ☆ + URL field) and an empty state with a URL field and Recents (`cursor-sp-14-browser-again.png`) |
| `2026-09-17-22` | *I want a feature update for the way all items get opened files, browsers, terminals, sub chats. The should all open as side panes which are side displayued with the main chat.* | the sorting-workers chat; the request itself is the design you are building |
| `2026-09-17-24` | *No clear way to close the project page. SHould follow previous issue I reported where opened panels are side panels next to the main chat* | the project page open in the column with its only close being the small × at the top-left header; he expected a tab with its own close |

Two more from the same batch touch your surface at the edge and stay with the desktop loop unless you want them: `-25` *"this three dots in the top right corner is not required remove it"* — the chat header's ⋯ (Copy / Fork / Archive / Delete); Cursor's chat header also has a ⋯ beside the panel toggle, so this is a product call the coordinator is confirming with him. And `-14` (front matter shown as prose in the document view) is fixed on the desktop side already.

Nothing here is changed by me; the panel's default open state, the browser surface and the project page's close all move to whatever the tabbed panel does.

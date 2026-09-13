# A-01b artifact cards — QA note (features agent, 2026-09-13)

Branch `cursor/artifact-cards-b027`, base `rust` (desktop only; pairs with kernel PRs #16 screenshot and #39 record).

## What it does

When a tool call produces files for the user, the desktop paints an **artifacts row** under the call: one card per file.

- A **picture** (`screenshot`, `browser` shot): the thumbnail, the file name, and the tool's measurements (`1920x1200 · 2716 KB`) as a caption.
- A **clip** (`record stop`): the last frame with a play badge, the file name, `10.2s · 177 KB`.
- Click a card → the file opens in the system viewer (`xdg-open` on Linux, `open` on macOS).

Source: `ToolRec.images` (pictures) and `ToolRec.paths` (a video extension makes a clip card; the call's image becomes its poster). Only `screenshot`, `record`, `browser` produce cards — a `read` of a PNG is looking, not making. The row is a new `ChatItem::Artifacts`, saved with the chat like every other item.

Driver: `item_json` reports `{"kind":"artifacts","files":[{kind,path,name,label,thumb}]}` so a test can assert what was shown without pixels. Each card has element id `artifact-<chat>-<ix>-<n>`; the row is `artifacts-<chat>-<ix>`.

## Attack ideas

1. Screenshot 4000x3000: thumbnail must be capped (card ≤ 296x180), history file must not grow by the full PNG (thumbnail is ≤ 880x640 PNG re-encoded).
2. Record stop with no poster (ffmpeg missing): card shows the "Recording" placeholder with the play badge, still clickable.
3. Path deleted after the turn: click does nothing harmful (the OS viewer shows its own error); card stays.
4. Six screenshots in one turn: row wraps, nothing overflows the reading column at 900 px window width.
5. `browser screenshot` today: does its shot come through `images`? If yes a card appears; if the kernel keeps it on a surface only, no double picture.
6. Reopen the chat (desktop restart): cards come back from the saved items with thumbnails; kernel replay of the same transcript must not add a second row per call.
7. Dark mode: border and caption contrast.
8. A failed `screenshot` call (no display): no row.
9. Child session (sub-agent) screenshots: row appears in the child's chat, not the parent's.
10. Click the card during a live turn: opener launches, transcript keeps following.

## How to run

Desktop on `:1` with the #39 kernel (`ARBOS_KERNEL_BIN`), prompt: "take a screenshot of the screen; then record start, sleep 4 in bash, record stop". Expect two cards; driver snapshot `items[-1].kind == "artifacts"` with one image and one video file.

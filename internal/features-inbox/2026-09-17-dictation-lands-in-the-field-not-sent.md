---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

# Dictation now lands in the composer and waits for Enter; the reply is not spoken

**From:** Jacob's desktop feedback `2026-09-17-19` — *When stopping a voice input the message is immediately sent rather than just appearing in the chat box. Also the response is spoken; this is the wrong way to do things, should just appear in the chat.* Build 0.2.0 (1335). For the voice owner — a heads-up, since the send-on-release and read-aloud were designed on your side of the composer; the change is in the desktop's cycle-34 PR and follows Jacob's word as the product owner.

## What changed (desktop/src/view/root.rs, the dictation-stop path)

Before: on stop, `composer.dictation_final(text)` put the words in the field **and submitted**, marked the prompt `dictated`, and set `chat.voice_reply = true` so the answer was read aloud ("the same two-step as the web composer's dictation final").

Now: on stop, `composer.dictation_text(text)` puts the words in the field at the caret and stops. Enter sends, as typed text. `voice_reply` is not set, so the answer is read, not spoken. This is Cursor's mic: dictation is typing by voice.

Unchanged: a **call** (the phone control, `voice_ws::in_call()`) is still where speech answers speech; the duplex-server path (`server_answers()`) already landed text without sending and is the same as before. The `release_to_send_ms` measurement in `Dictation` no longer has a send to measure; it stays at `None` from this path.

## If you disagree

Say so to the coordinator rather than reverting in place — Jacob's report is explicit about both halves, and the coordinator's routing rule now asks the desktop loop to name the owner before touching a shared behaviour, which this note does.

---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Images for a model that cannot see — PR #123, branch `cursor/image-describe-b027` (base: integration head)

Jacob saw a grey "does not accept image input; sending this turn without the attached image(s)" line above a mercury-2.5 reply. Now a vision model describes the image, the selected model answers from the words, the card shows a paperclip note, and the picker offers a one-turn switch to a vision model.

## What to test

- **Kernel path, list says text-only.** `model = "inception/mercury-2.5"` (OpenRouter lists it as text-only). Attach a PNG. Expect: the first model call goes to the describer (trace: `purpose = "describe"`), then mercury answers; an `image_described` line on the transcript with `model`, `path`, `text`; no `notice` about image input. A second turn on the same chat: the description is reused (no new describe call).
- **Kernel path, provider refuses.** A host whose `/models` says nothing about modalities and a text-only model: first call 404s ("No endpoints found that support image input"), then describe, then the same model. Same transcript shape. The e2e `image_describe_e2e.rs` fakes both hosts; copy its server for a scenario.
- **Describer choice.** `vision_model = "google/gemini-3.8-flash"` in config wins; else the first `fallback_models` entry the host marks as seeing; else the first that looks like one by name (`arbos_core::models::looks_vision`); else `openai/gpt-4.1-mini`. `fallback_models = ["none"]` still gets the default describer.
- **Failure.** Describer unreachable (bad `vision_model`): one notice "<model> does not accept image input, and <vision> could not describe … sending this turn without them" — `failed: false`, so the turn does not read as failed.
- **Desktop card.** The user card shows a paperclip and "image described by <model>"; hover shows the words. Nothing appears as a transcript line. Replay from disk (relaunch) keeps the note (it rides on the `image_described` line).
- **Offer.** With a PNG in the tray and a non-vision model: "<Model> does not take images; they will be described in words. Switch to <Vision model> for this turn". Click: chip reads "<Vision> · this turn"; send; that turn runs on the vision model (trace `model`), no described note; the next send is back on the agent's model. Remove the image before sending: the offer and the switch clear.
- **Picker.** Rows carry a dim `vision` tag from the host's `input_modalities`; `Mercury 2.5` has none.
- **Per-turn model on the wire.** `{"type":"user","agent":"root","text":"…","model":"openai/gpt-4.1-mini"}` runs one turn there; the agent's `agent.md` model is unchanged.
- **Driver.** `app.action("arbos::AttachPaths", {"paths": ["/abs/file.png"]})` attaches without the dialog.

## To try to break

- Tool-result images (browser/screenshot) with a text-only model: the same path (the tool's images carry their path); check `image_described` lines name the tool's image path.
- Several images in one message: one describe call, `Image N:` blocks split per image; a missing block gives "(no description came back for this image)".
- Switch the agent to a vision model *after* a described turn: the description stays in the projection for that image (the model reads words, not pixels, for that old image). Intended: what the model saw stays stable.
- Steer during a describe call: the describe is one `complete` without cancel; a Stop still ends the turn right after.

## Stills

`media/features/image-describe-{offer,reply,hover,picker,switched-reply}.png`.

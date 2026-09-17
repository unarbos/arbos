# M-03b cost per turn + thinking-block retry — QA note (features agent, 2026-09-13)

Branch `cursor/cost-per-turn-b027`, base `rust`.

## What it does

1. **Cost.** OpenRouter requests carry `usage: {include: true}`, so each streamed call ends with `usage.cost` (dollars). `Completion.cost` → `Usage.cost` on the step → summed over the turn → `turn_complete.usage.cost` in the transcript (`{"used":3159,"size":400000,"cost":0.00503175}`). The desktop's `UsageUpdate` carries it as `Cost {amount, currency: "USD"}`; `ChatSession.usage` gains `spent` (chat total since open) and `last_cost`; the model card popover (click the model chip) shows a **Cost** row under **Context**: `$0.0050`, tooltip "last turn $…". Driver `usage` gains `spent`/`last_cost`.
2. **Thinking-block retry.** When a provider answers 400/422 (or a stream error) whose message names the thinking blocks ("thought signature", "reasoning_details", "thinking … signature/invalid"), the same model is asked once more with every stored `reasoning_details` stripped from the messages, with a Notice. Only once per step; after that the normal retry/fallback verdict applies.

## Attack ideas

1. OpenAI direct (`api.openai.com`): no `usage.cost`; Cost row must not appear; `turn_complete.usage` has no `cost` key.
2. A turn with 6 model steps: `cost` is the sum, not the last step. Compare with OpenRouter's activity page for the same generation ids.
3. A turn that ends in Interrupted/Cut: `end(None, …)` drops the cost for that turn — spent under-counts. Note it.
4. Desktop reopen: `spent` is runtime-only; on reconnect the kernel replays `turn_complete` events through `kernel_event`, so `spent` should rebuild to the full history. Verify it does not double-count (replay + live).
5. Fallback mid-turn (#34): cost sums across models. Fine, but the tooltip says nothing about which model. Note.
6. Force the thinking-block path: replay a transcript with a Gemini assistant message whose `reasoning_details` signature is edited to garbage, then prompt. Expect one Notice "rejected the stored thinking blocks … retrying once without them" and a normal answer.
7. Same error twice in a row (provider keeps rejecting): second time goes to the normal verdict (Fail or Fallback), no loop.
8. An unrelated 400 mentioning "thinking" in the user's own text echoed by the provider: false positive would strip reasoning and retry once — harmless but note it.
9. `dollars()` formatting: 0.00001 → `$0.0000`; 0.5 → `$0.500`; 12.3456 → `$12.35`.
10. Free models (`:free` suffix): `cost: 0.0` → row shows `$0.0000`; decide if zero should hide.

## How to run

`arbos-kernel run . 'echo hi via bash, then one word'`; `grep turn_complete .arbos/agents/root/transcript.jsonl | tail -1` shows `cost`. Desktop: send a prompt, click the model chip, see the Cost row.

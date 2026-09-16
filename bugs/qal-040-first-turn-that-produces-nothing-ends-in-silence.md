# qal-040: a first turn whose model returns nothing ends with nothing for the user — no words, no notice

- Feature: the kickoff turn / empty-reply handling, `main` @ `c964294c` and `main`+#298 alike
- Severity: medium-high for a first-time user: the project opens, the kickoff turn runs, and the chat stays blank. Nothing says the model returned nothing, nothing says what to do. (The kickoff correctly does *not* count as taken — a second `kickoff` frame runs another turn — but nobody sends one.)
- **Closed 2026-09-16 12:05 UTC** by #303 (`main` @ `5ecba2d1`): two empty replies end with the notice "zeta/empty returned nothing twice. Check the model in Settings › Model (or fallback_models in config.toml)…", and with fallbacks the next model takes the turn. `fr-03` green on merged main.
- Scenario: `fr-03-first-turn-produces-nothing` (check `fr-03-silent-failure`); rollout `internal/qa/rollouts/20260916T104540Z-fr-03-first-turn-produces-nothing/`

## Repro

Provider stub whose model answers every request with an empty `content` (`zeta/empty` in `batch_scenarios.FallbackStub`); `model = "zeta/empty"`, no fallbacks. Fresh place; send `{"type": "kickoff", "agent": "root"}`.

## Expected

After the nudge and the retry still yield nothing, the turn ends with a **notice the user can read** — "the model returned an empty reply twice; check the model in Settings › Model or try again" — and, with fallbacks configured, the fallback is tried as it is for a 403 or a silent first byte (#298). The desktop shows that line where the greeting would have been.

## Actual

Transcript: `wake kickoff` → `nudge "Your reply was empty. Continue the task…"` → `turn_complete`. Three model calls, no assistant text, no notice. The only trace of what happened is a `nudge` line (kernel→model, not shown as a message).

## Suspected location

`crates/arbos-engine/src/turn.rs`, the empty-reply path after the nudge: when the retry is empty too, `end()` is reached without a `Notice`. Same place could route an all-empty primary to `fallback_models` (as `step.rs` does for 403/first-byte).

## Fix

PR #303 (`cursor/empty-reply-fallback-b027`, stacked on #298): the second empty reply in a row is a model failing. With a fallback configured, the next model takes the turn and the transcript says "<model> returned nothing twice, so <next> answers this turn."; alone, a failed notice: "<model> returned nothing twice. Check the model in Settings › Model (or fallback_models in config.toml)." — on a kickoff, "Nothing was set up yet; your first message starts the project as usual." The kickoff stays not-taken, as before. Also from the scenarios: an unprefixed model id is now its own blocked family, so a 403 on `gpt-4.1-mini` is remembered. E2e `fallback_403_e2e::two_empty_replies_go_to_the_fallback_or_end_with_a_readable_notice`. Re-run `fr-03-first-turn-produces-nothing`: expect the failed notice in place of the greeting, and `fr-03-silent-failure` green.

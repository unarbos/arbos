package core

import "strings"

// Project folds a session's event log into the provider-facing conversation.
// It is a pure function and lives here, next to the Event and Message types it
// maps between, so:
//
//   - the EventKind -> Message mapping has one obvious home;
//   - it is testable and fuzzable with no engine, store, or actor;
//   - the engine never switches over concrete event kinds for projection.
//
// The projected conversation has three regions, in order:
//
//  1. the base system prompt (stable);
//  2. the conversation, with compressed spans folded away and their summaries
//     rendered in place at the span start (append-only → stable prefix);
//  3. an injected-context block (memory/jobs/skills/retrieval) — the latest
//     segment per source, fenced.
//
// The injected-context block is the trailing suffix, NOT a prefix: its content
// changes across turns (memory recall, the plan forest, the jobs table), and a
// volatile block placed ahead of the conversation would invalidate the prompt
// cache for all history behind it. Rendering it last keeps the longest possible
// byte-stable prefix (system + append-only conversation) cacheable, and pushes
// the only per-turn variation to the end (the cache-aware layout). The engine's
// live request composes the same way (ProjectConversation + ProjectContext), so
// a replay via Project and a live turn stay byte-consistent.
//
// Unknown/future payloads are ignored rather than rejected: a newer log read by
// an older binary (or vice versa, post-upcasting) must still project cleanly.
func Project(events []Event, systemPrompt string) []Message {
	msgs := make([]Message, 0, len(events)+2)
	if systemPrompt != "" {
		msgs = append(msgs, Message{Role: RoleSystem, Content: systemPrompt})
	}

	msgs = appendConversation(msgs, events)
	msgs = appendContextBlock(msgs, events)
	return msgs
}

// ProjectContext renders only the injected-context block — the latest segment
// per source, fenced — and nothing else. The engine composes it as the trailing
// suffix of a live request (after the conversation prefix it caches), so the
// volatile memory/plan/jobs context never sits inside the cacheable prefix. It
// returns nil when no context has been injected.
func ProjectContext(events []Event) []Message {
	return appendContextBlock(nil, events)
}

// ProjectConversation renders only the conversation region of Project — no
// system prompt, no injected-context block — folding compressed spans the same
// way. The engine's summarizer uses it to fold a span that may already contain
// a summary: the earlier summary renders in place of its raw events, so each
// re-summarization's input is bounded by new material, not total session
// history.
func ProjectConversation(events []Event) []Message {
	return appendConversation(make([]Message, 0, len(events)), events)
}

// ProjectEvent maps a single event to its conversation Message in isolation —
// ignoring compression folding and the injected-context block, which are
// inherently multi-event and handled by Project. ok is false when the event has
// no standalone conversational projection (compression/context/usage/interrupt).
//
// This is the single home for the per-event Role/field mapping: both Project's
// conversation pass and the engine's incremental in-turn extension call it, so a
// live turn and a replay from the log cannot drift even as Message grows.
func ProjectEvent(e Event) (Message, bool) {
	// Switch on the Kind discriminator, not the Go type: the `exhaustive`
	// linter checks EventKind switches, so adding a payload kind without
	// deciding its projection fails CI — a type switch would let the new
	// payload silently vanish from every conversation. The trailing return
	// covers kinds from newer writers (ADR-0010: ignore unknown, never fail).
	switch e.Payload.Kind() {
	case EventUserMessage, EventAssistantMessage:
		return e.Payload.(MessagePayload).Message, true
	case EventToolResult:
		r := e.Payload.(ToolResultPayload).Result
		return Message{Role: RoleTool, ToolCallID: r.CallID, Content: r.Content, Parts: r.Blocks}, true
	case EventCompressed, EventContext, EventUsage, EventInterrupted, EventConfig:
		return Message{}, false
	}
	return Message{}, false
}

// appendContextBlock renders injected context as a stable fenced prefix. Only
// the latest segment per source is materialized (older injections are
// superseded), so the prompt does not grow with every turn's memory recall.
// Sources render in first-seen order for stability across turns.
func appendContextBlock(msgs []Message, events []Event) []Message {
	latest := map[string]string{}
	var order []string
	for i := range events {
		p, ok := events[i].Payload.(ContextPayload)
		if !ok {
			continue
		}
		for _, seg := range p.Segments {
			if _, seen := latest[seg.Source]; !seen {
				order = append(order, seg.Source)
			}
			latest[seg.Source] = seg.Content
		}
	}
	for _, src := range order {
		msgs = append(msgs, Message{Role: RoleSystem, Content: fence(src, latest[src])})
	}
	return msgs
}

// appendConversation renders user/assistant/tool turns, folding any span
// replaced by a CompressionPayload and emitting the summary at the span's start.
func appendConversation(msgs []Message, events []Event) []Message {
	type span struct{ lo, hi int64 }
	var ranges []span
	for i := range events {
		if p, ok := events[i].Payload.(CompressionPayload); ok {
			ranges = append(ranges, span{p.ReplacedSeqLo, p.ReplacedSeqHi})
		}
	}
	covered := func(seq int64) bool {
		for _, r := range ranges {
			if seq >= r.lo && seq <= r.hi {
				return true
			}
		}
		return false
	}

	// A summary renders at its span's lo, unless that lo sits inside a strictly
	// larger span — i.e. a later re-compression subsumed this one, so the bigger
	// summary supersedes it.
	summaryAtLo := map[int64]string{}
	for i := range events {
		p, ok := events[i].Payload.(CompressionPayload)
		if !ok {
			continue
		}
		subsumed := false
		for j := range ranges {
			r := ranges[j]
			if p.ReplacedSeqLo >= r.lo && p.ReplacedSeqLo <= r.hi && (r.hi-r.lo) > (p.ReplacedSeqHi-p.ReplacedSeqLo) {
				subsumed = true
				break
			}
		}
		if !subsumed {
			summaryAtLo[p.ReplacedSeqLo] = p.Summary
		}
	}

	for i := range events {
		ev := events[i]
		if sum, ok := summaryAtLo[ev.Seq]; ok {
			msgs = append(msgs, Message{Role: RoleSystem, Content: sum})
		}
		// Compressed spans are folded away; their summary was emitted above.
		if covered(ev.Seq) {
			continue
		}
		// The per-event mapping lives in ProjectEvent (the single home). Context,
		// usage, and interrupt events have no standalone projection and are
		// skipped here (context is rendered by appendContextBlock above).
		if m, ok := ProjectEvent(ev); ok {
			msgs = append(msgs, m)
		}
	}
	return msgs
}

// fence wraps injected content in provenance markers so the model can tell
// injected context from conversation, mitigating prompt-injection that tries to
// impersonate system instructions.
func fence(source, content string) string {
	var b strings.Builder
	b.WriteString("<<")
	b.WriteString(source)
	b.WriteString(">>\n")
	b.WriteString(content)
	b.WriteString("\n<</")
	b.WriteString(source)
	b.WriteString(">>")
	return b.String()
}

package core

// ReasoningLevel is the provider-neutral reasoning/thinking effort for a
// request. Adapters map it to their native parameter (e.g. reasoning_effort);
// an empty level means "provider default". The set mirrors pi's thinking levels.
type ReasoningLevel string

const (
	ReasoningOff     ReasoningLevel = "off"
	ReasoningMinimal ReasoningLevel = "minimal"
	ReasoningLow     ReasoningLevel = "low"
	ReasoningMedium  ReasoningLevel = "medium"
	ReasoningHigh    ReasoningLevel = "high"
	ReasoningXHigh   ReasoningLevel = "xhigh"
)

// CacheRetention is the provider-neutral prompt-cache retention hint. Adapters
// that support prompt caching map it to their native mechanism; an empty value
// means "provider default" (pi's default is "short"). Mirrors pi's set.
type CacheRetention string

const (
	CacheNone  CacheRetention = "none"
	CacheShort CacheRetention = "short"
	CacheLong  CacheRetention = "long"
)

// LLMRequest is the provider-neutral request the engine builds each iteration.
// Provider adapters translate this into their native API; the engine never
// constructs provider-specific payloads. It is built fresh per iteration and
// never persisted, so the reasoning/caching fields are additive with no log
// shape impact.
type LLMRequest struct {
	Model       string
	Messages    []Message
	Tools       []ToolSchema
	Stream      bool
	Temperature *float64 // nil = provider default
	MaxTokens   int      // 0 = provider default

	// Reasoning is the requested reasoning effort; "" = provider default.
	Reasoning ReasoningLevel
	// WebSearch asks the provider to give the model real-time web search for
	// this request. It is a provider-neutral intent: an adapter maps it to its
	// native mechanism (the openai adapter to OpenRouter's web_search server
	// tool, a native Anthropic/Gemini adapter to their built-in search), and
	// drops it when the endpoint has no search. The search runs provider-side;
	// the client dispatches nothing and receives the sources as Message
	// citations. See ADR-0027.
	WebSearch bool
	// WebFetch asks the provider to let the model retrieve full page content
	// from a URL (server-side extraction), the natural complement to WebSearch
	// for reading pages the model finds. Same neutral-intent contract: the
	// openai adapter maps it to OpenRouter's web_fetch server tool and it is
	// dropped where unsupported. It is independent of WebSearch — either can be
	// enabled alone — and the local fetch tool stays available regardless.
	WebFetch bool
	// ImageGen asks the provider to let the model generate images for this
	// request. Same neutral-intent contract as WebSearch/WebFetch: the openai
	// adapter maps it to OpenRouter's image_generation server tool (the model
	// invokes it, OpenRouter runs it server-side, and the images ride back on
	// the response), and it is dropped where unsupported. The generated images
	// land on the assistant Message's Parts.
	ImageGen bool
	// CacheRetention is the prompt-cache retention hint; "" = provider default.
	CacheRetention CacheRetention
	// SessionID is a stable id for providers that key prompt caching or routing
	// on a session. The engine sets it from the conversation id; "" disables it.
	SessionID string
}

// ToolCallProgress is a snapshot of a tool call still streaming its arguments:
// the call's identity (known from the stream's block-start) and how many
// argument bytes have arrived so far. An adapter emits a series of these as a
// large call (e.g. a write of a whole HTML canvas) is composed, so the frontend
// can show live progress in the gap between the prose and the finished call.
// Nothing here is persisted.
type ToolCallProgress struct {
	ID    string
	Name  string
	Bytes int
	// ArgsDelta is the raw argument-JSON fragment that just arrived (not the
	// cumulative buffer), so a frontend can accumulate it and render the call's
	// content streaming in — e.g. the file body of a write appearing line by
	// line. Empty on the block-start ping that only announces the call.
	ArgsDelta string
}

// LLMChunk is one streamed unit from a provider. A provider emits a sequence of
// chunks and closes the channel when the response is complete. Deltas are
// additive; ToolCalls and Usage arrive fully-formed (the adapter is responsible
// for accumulating any partial tool-call fragments before emitting), while
// ToolProgress reports those fragments accumulating, for live UI only.
type LLMChunk struct {
	ContentDelta   string
	ReasoningDelta string
	ToolCalls      []ToolCall
	// ToolProgress, when non-nil, reports a tool call's arguments growing
	// mid-stream. It is presentation-only — the engine forwards it to the
	// frontend but never logs it — so it is runtime-only, never serialized.
	ToolProgress *ToolCallProgress `json:"-"`
	// Citations are web-search sources the provider attached to this response.
	// Like ToolCalls they arrive fully-formed (the adapter accumulates any
	// streamed annotation fragments before emitting); the engine collects them
	// onto the assistant Message. Runtime-only; never serialized on the chunk.
	Citations []Citation `json:"-"`
	// Images are provider-generated images attached to this response (e.g.
	// OpenRouter's image_generation server tool). Like Citations they arrive
	// fully-formed; the engine collects them onto the assistant Message's
	// Parts. Runtime-only; never serialized on the chunk.
	Images []ContentBlock `json:"-"`
	Usage  *Usage
	// Err reports a mid-stream failure (a truncated/aborted response, an
	// over-long line) the adapter discovered after Stream already returned its
	// channel. Without it a broken stream closes silently and looks like a clean
	// completion. The engine surfaces a non-nil Err as a retryable provider
	// error and aborts the turn. Runtime-only; never serialized.
	Err  error `json:"-"`
	Done bool
	// FinishReason is the provider's stop reason on the final chunk (e.g.
	// "stop", "length", "tool_calls"). Runtime-only; never serialized.
	FinishReason string `json:"-"`
}

// Usage reports token accounting for a completed response.
type Usage struct {
	PromptTokens     int
	CompletionTokens int
	TotalTokens      int
}

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
	// CacheRetention is the prompt-cache retention hint; "" = provider default.
	CacheRetention CacheRetention
	// SessionID is a stable id for providers that key prompt caching or routing
	// on a session. The engine sets it from the conversation id; "" disables it.
	SessionID string
}

// LLMChunk is one streamed unit from a provider. A provider emits a sequence of
// chunks and closes the channel when the response is complete. Deltas are
// additive; ToolCalls and Usage arrive fully-formed (the adapter is responsible
// for accumulating any partial tool-call fragments before emitting).
type LLMChunk struct {
	ContentDelta   string
	ReasoningDelta string
	ToolCalls      []ToolCall
	Usage          *Usage
	// Err reports a mid-stream failure (a truncated/aborted response, an
	// over-long line) the adapter discovered after Stream already returned its
	// channel. Without it a broken stream closes silently and looks like a clean
	// completion. The engine surfaces a non-nil Err as a retryable provider
	// error and aborts the turn. Runtime-only; never serialized.
	Err  error `json:"-"`
	Done bool
}

// Usage reports token accounting for a completed response.
type Usage struct {
	PromptTokens     int
	CompletionTokens int
	TotalTokens      int
}

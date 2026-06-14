package core

// Role identifies the author of a message in provider-neutral terms.
type Role string

const (
	RoleSystem    Role = "system"
	RoleUser      Role = "user"
	RoleAssistant Role = "assistant"
	RoleTool      Role = "tool"
)

// Message is the projection unit handed to a provider. It is intentionally
// provider-neutral; each LLMProvider adapter is responsible for translating
// to/from its own wire format.
type Message struct {
	Role    Role
	Content string
	// Parts carries non-text content (images) for a multimodal message, in
	// order. It is additive: Content stays the canonical text channel, so a
	// text-only message leaves Parts nil and projects/serializes exactly as
	// before. A consumer that wants the full multimodal view treats Content as a
	// leading text block followed by Parts. This mirrors the
	// `string | (text|image)[]` content union real providers expose. See ADR-0022.
	Parts      []ContentBlock `json:",omitempty"`
	Reasoning  string         // assistant reasoning/thinking trace text, when provided
	ToolCalls  []ToolCall     // set on assistant messages that request tools
	ToolCallID string         // set on RoleTool messages, links back to a ToolCall
	// Origin names the door a user message arrived through (a web connection's
	// "web:<n>", a Telegram chat's id). It lets every other door recognize and
	// suppress the echo of a message it sent itself, without matching content.
	// Empty for assistant/tool/system messages and for the local terminal. It
	// is presentation provenance, not part of the provider projection.
	Origin string `json:",omitempty"`
	// Citations are web-search sources the provider grounded this message on
	// (an assistant turn). They are additive: a turn with no web search leaves
	// them nil and serializes exactly as before. Unlike a tool call, a citation
	// is not dispatched — the provider runs the search server-side (e.g.
	// OpenRouter's web_search server tool) and reports the sources it used. See
	// ADR-0027.
	Citations []Citation `json:",omitempty"`
}

// Citation is one web-search source a provider used to ground an assistant
// message. It is provider-neutral: each adapter maps its native annotation
// shape (OpenRouter standardizes on OpenAI's url_citation) into this. Start/End
// are character offsets into Message.Content that the cited span backs, when
// the provider reports them (0,0 = unanchored).
type Citation struct {
	URL        string `json:"url"`
	Title      string `json:"title,omitempty"`
	Content    string `json:"content,omitempty"`
	StartIndex int    `json:"start_index,omitempty"`
	EndIndex   int    `json:"end_index,omitempty"`
}

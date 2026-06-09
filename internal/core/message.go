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
}

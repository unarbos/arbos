package anthropic

import (
	"encoding/json"
	"strings"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/provider/internal/providerkit"
)

// Wire types: the JSON shapes of Anthropic's Messages API. Only the fields the
// kernel needs are modeled.

type wireRequest struct {
	Model     string        `json:"model"`
	MaxTokens int           `json:"max_tokens"`
	System    string        `json:"system,omitempty"`
	Messages  []wireMessage `json:"messages"`
	Tools     []wireTool    `json:"tools,omitempty"`
	Stream    bool          `json:"stream"`
	Thinking  *wireThinking `json:"thinking,omitempty"`
}

type wireThinking struct {
	Type         string `json:"type"` // "enabled"
	BudgetTokens int    `json:"budget_tokens"`
}

type wireMessage struct {
	Role    string      `json:"role"`
	Content []wireBlock `json:"content"`
}

type wireBlock struct {
	Type string `json:"type"` // text | image | document | tool_use | tool_result
	Text string `json:"text,omitempty"`
	// image | document
	Source *wireSource `json:"source,omitempty"`
	// tool_use
	ID    string          `json:"id,omitempty"`
	Name  string          `json:"name,omitempty"`
	Input json.RawMessage `json:"input,omitempty"`
	// tool_result
	ToolUseID string `json:"tool_use_id,omitempty"`
	Content   string `json:"content,omitempty"`
	IsError   bool   `json:"is_error,omitempty"`
}

// wireSource is the base64 payload of an image or document block. Anthropic
// parses a document (PDF) natively, including OCR for scanned pages.
type wireSource struct {
	Type      string `json:"type"`       // "base64"
	MediaType string `json:"media_type"` // e.g. image/png, application/pdf
	Data      string `json:"data"`
}

type wireTool struct {
	Name        string          `json:"name"`
	Description string          `json:"description,omitempty"`
	InputSchema json.RawMessage `json:"input_schema"`
}

// wireEvent is the streamed SSE payload (discriminated by Type, mirroring the
// event name).
type wireEvent struct {
	Type    string `json:"type"`
	Index   int    `json:"index"`
	Message *struct {
		Usage *struct {
			InputTokens  int `json:"input_tokens"`
			OutputTokens int `json:"output_tokens"`
		} `json:"usage"`
	} `json:"message"`
	ContentBlock *struct {
		Type string `json:"type"`
		ID   string `json:"id"`
		Name string `json:"name"`
	} `json:"content_block"`
	Delta *struct {
		Type        string `json:"type"`
		Text        string `json:"text"`
		PartialJSON string `json:"partial_json"`
		Thinking    string `json:"thinking"`
		StopReason  string `json:"stop_reason"`
	} `json:"delta"`
	Usage *struct {
		OutputTokens int `json:"output_tokens"`
	} `json:"usage"`
}

// buildRequest converts the neutral request to the Messages body. The system
// prompt is hoisted out of messages; consecutive tool results group into one
// user turn (Anthropic requires tool_result blocks in the user turn following
// the assistant's tool_use).
func (p *Provider) buildRequest(req core.LLMRequest) wireRequest {
	w := wireRequest{Model: req.Model, Stream: req.Stream}
	w.MaxTokens = req.MaxTokens
	if w.MaxTokens <= 0 {
		w.MaxTokens = defaultMaxTokens
	}
	if budget := thinkingBudget(req.Reasoning); budget > 0 {
		w.Thinking = &wireThinking{Type: "enabled", BudgetTokens: budget}
		if w.MaxTokens <= budget {
			w.MaxTokens = budget + defaultMaxTokens
		}
	}

	var system []string
	for _, m := range req.Messages {
		switch m.Role {
		case core.RoleSystem:
			if m.Content != "" {
				system = append(system, m.Content)
			}
		case core.RoleUser:
			w.Messages = append(w.Messages, wireMessage{Role: "user", Content: userBlocks(m)})
		case core.RoleAssistant:
			var blocks []wireBlock
			if m.Content != "" {
				blocks = append(blocks, wireBlock{Type: "text", Text: m.Content})
			}
			for _, tc := range m.ToolCalls {
				blocks = append(blocks, wireBlock{Type: "tool_use", ID: tc.ID, Name: tc.Name, Input: tc.ArgsOrEmpty()})
			}
			w.Messages = append(w.Messages, wireMessage{Role: "assistant", Content: blocks})
		case core.RoleTool:
			content := m.Content
			if len(m.Parts) > 0 {
				content = providerkit.ToolResultText(m)
			}
			result := wireBlock{Type: "tool_result", ToolUseID: m.ToolCallID, Content: content}
			// Image/file parts ride as real image/document blocks in the SAME
			// user turn — Anthropic accepts them alongside tool_result — so a
			// screenshot or a read image reaches vision instead of being dropped.
			var media []wireBlock
			for _, b := range providerkit.ToolResultVisionParts(m) {
				if wb, ok := mediaBlock(b); ok {
					media = append(media, wb)
				}
			}
			// Group with the previous turn if it is already a tool-result user
			// turn. Anthropic requires EVERY tool_result block to precede any
			// other block in the turn (400 otherwise — and a persisted bad
			// result would re-project the same malformed turn forever). So a
			// grouped result inserts after the existing tool_results, and media
			// always trails the turn.
			if n := len(w.Messages); n > 0 && w.Messages[n-1].Role == "user" && isToolResultTurn(w.Messages[n-1]) {
				prev := w.Messages[n-1].Content
				idx := 0
				for idx < len(prev) && prev[idx].Type == "tool_result" {
					idx++
				}
				merged := make([]wireBlock, 0, len(prev)+1+len(media))
				merged = append(merged, prev[:idx]...)
				merged = append(merged, result)
				merged = append(merged, prev[idx:]...)
				merged = append(merged, media...)
				w.Messages[n-1].Content = merged
			} else {
				w.Messages = append(w.Messages, wireMessage{Role: "user", Content: append([]wireBlock{result}, media...)})
			}
		}
	}
	if len(system) > 0 {
		w.System = strings.Join(system, "\n\n")
	}
	for _, ts := range req.Tools {
		w.Tools = append(w.Tools, wireTool{Name: ts.Name, Description: ts.Description, InputSchema: ts.Parameters})
	}
	return w
}

// userBlocks renders a user message as Anthropic content blocks: the typed text
// first, then each multimodal part as an image or document block. A plain
// text-only message yields a single text block, exactly as before.
func userBlocks(m core.Message) []wireBlock {
	blocks := make([]wireBlock, 0, len(m.Parts)+1)
	if m.Content != "" || len(m.Parts) == 0 {
		blocks = append(blocks, wireBlock{Type: "text", Text: m.Content})
	}
	for _, b := range m.Parts {
		if b.Type == core.BlockText {
			if b.Text != "" {
				blocks = append(blocks, wireBlock{Type: "text", Text: b.Text})
			}
			continue
		}
		if wb, ok := mediaBlock(b); ok {
			blocks = append(blocks, wb)
		}
	}
	return blocks
}

// mediaBlock renders an image or file content block as its Anthropic wire block
// (image / document, base64 source). The second return is false for a block
// that is not media or whose payload is missing.
func mediaBlock(b core.ContentBlock) (wireBlock, bool) {
	switch b.Type {
	case core.BlockImage:
		if b.Image != nil {
			return wireBlock{Type: "image", Source: &wireSource{
				Type: "base64", MediaType: b.Image.MimeType, Data: b.Image.Data,
			}}, true
		}
	case core.BlockFile:
		if b.File != nil {
			return wireBlock{Type: "document", Source: &wireSource{
				Type: "base64", MediaType: b.File.MimeType, Data: b.File.Data,
			}}, true
		}
	default:
		// Non-media blocks (e.g. text) carry no wire media block.
	}
	return wireBlock{}, false
}

// isToolResultTurn reports whether a user turn is a tool-result turn that a
// following tool result may join. It holds at least one tool_result and only
// tool_result plus promoted media (image/document) blocks — never text or
// tool_use — so grouping survives an earlier result that carried a screenshot.
func isToolResultTurn(m wireMessage) bool {
	hasResult := false
	for _, b := range m.Content {
		switch b.Type {
		case "tool_result":
			hasResult = true
		case "image", "document":
		default:
			return false
		}
	}
	return hasResult
}

// thinkingBudget maps a neutral reasoning level to an Anthropic thinking budget
// (tokens). off/"" disables thinking.
func thinkingBudget(r core.ReasoningLevel) int {
	switch r {
	case core.ReasoningMinimal:
		return 1024
	case core.ReasoningLow:
		return 2048
	case core.ReasoningMedium:
		return 4096
	case core.ReasoningHigh:
		return 8192
	case core.ReasoningXHigh:
		return 16384
	default:
		return 0
	}
}

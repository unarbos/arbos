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
	Type string `json:"type"` // text | tool_use | tool_result
	Text string `json:"text,omitempty"`
	// tool_use
	ID    string          `json:"id,omitempty"`
	Name  string          `json:"name,omitempty"`
	Input json.RawMessage `json:"input,omitempty"`
	// tool_result
	ToolUseID string `json:"tool_use_id,omitempty"`
	Content   string `json:"content,omitempty"`
	IsError   bool   `json:"is_error,omitempty"`
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
			w.Messages = append(w.Messages, wireMessage{Role: "user", Content: []wireBlock{{Type: "text", Text: m.Content}}})
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
			// tool_result content is text-only here; downgrade image parts to a
			// placeholder so an image tool result is not silently lost.
			content := m.Content
			if len(m.Parts) > 0 {
				content = providerkit.ToolResultText(m)
			}
			block := wireBlock{Type: "tool_result", ToolUseID: m.ToolCallID, Content: content}
			// Group with the previous turn if it is already a tool-result user turn.
			if n := len(w.Messages); n > 0 && w.Messages[n-1].Role == "user" && isToolResultTurn(w.Messages[n-1]) {
				w.Messages[n-1].Content = append(w.Messages[n-1].Content, block)
			} else {
				w.Messages = append(w.Messages, wireMessage{Role: "user", Content: []wireBlock{block}})
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

func isToolResultTurn(m wireMessage) bool {
	for _, b := range m.Content {
		if b.Type != "tool_result" {
			return false
		}
	}
	return len(m.Content) > 0
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

package google

import (
	"encoding/json"
	"strings"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/provider/internal/providerkit"
)

// Wire types: the JSON shapes of Gemini's generateContent API. Only the fields
// the kernel needs are modeled.

type wireRequest struct {
	Contents          []wireContent      `json:"contents"`
	Tools             []wireTool         `json:"tools,omitempty"`
	SystemInstruction *wireContent       `json:"systemInstruction,omitempty"`
	GenerationConfig  *wireGenerationCfg `json:"generationConfig,omitempty"`
}

type wireGenerationCfg struct {
	MaxOutputTokens int                 `json:"maxOutputTokens,omitempty"`
	ThinkingConfig  *wireThinkingConfig `json:"thinkingConfig,omitempty"`
}

type wireThinkingConfig struct {
	ThinkingBudget int `json:"thinkingBudget"`
}

type wireContent struct {
	Role  string     `json:"role,omitempty"`
	Parts []wirePart `json:"parts"`
}

type wirePart struct {
	Text             string                `json:"text,omitempty"`
	FunctionCall     *wireFunctionCall     `json:"functionCall,omitempty"`
	FunctionResponse *wireFunctionResponse `json:"functionResponse,omitempty"`
}

type wireFunctionCall struct {
	Name string          `json:"name"`
	Args json.RawMessage `json:"args,omitempty"`
}

type wireFunctionResponse struct {
	Name     string         `json:"name"`
	Response map[string]any `json:"response"`
}

type wireTool struct {
	FunctionDeclarations []wireFunctionDecl `json:"functionDeclarations"`
}

type wireFunctionDecl struct {
	Name        string          `json:"name"`
	Description string          `json:"description,omitempty"`
	Parameters  json.RawMessage `json:"parameters,omitempty"`
}

type wireResponse struct {
	Candidates []struct {
		Content wireContent `json:"content"`
	} `json:"candidates"`
	UsageMetadata *struct {
		PromptTokenCount     int `json:"promptTokenCount"`
		CandidatesTokenCount int `json:"candidatesTokenCount"`
		TotalTokenCount      int `json:"totalTokenCount"`
	} `json:"usageMetadata"`
}

// buildRequest converts the neutral request to a generateContent body. System
// messages hoist into systemInstruction; assistant tool calls become
// functionCall parts; tool results become functionResponse parts whose name is
// resolved from the originating tool call.
func (p *Provider) buildRequest(req core.LLMRequest) wireRequest {
	var w wireRequest

	// Map tool-call id -> function name so a later tool result can name its
	// functionResponse (Gemini matches by name, not id).
	callName := map[string]string{}

	var system []string
	for _, m := range req.Messages {
		switch m.Role {
		case core.RoleSystem:
			if m.Content != "" {
				system = append(system, m.Content)
			}
		case core.RoleUser:
			w.Contents = append(w.Contents, wireContent{Role: "user", Parts: []wirePart{{Text: m.Content}}})
		case core.RoleAssistant:
			var parts []wirePart
			if m.Content != "" {
				parts = append(parts, wirePart{Text: m.Content})
			}
			for _, tc := range m.ToolCalls {
				callName[tc.ID] = tc.Name
				parts = append(parts, wirePart{FunctionCall: &wireFunctionCall{Name: tc.Name, Args: tc.ArgsOrEmpty()}})
			}
			w.Contents = append(w.Contents, wireContent{Role: "model", Parts: parts})
		case core.RoleTool:
			name := callName[m.ToolCallID]
			if name == "" {
				name = m.ToolCallID
			}
			// functionResponse content is text-only; downgrade image parts to a
			// placeholder so an image tool result is not silently lost.
			content := m.Content
			if len(m.Parts) > 0 {
				content = providerkit.ToolResultText(m)
			}
			w.Contents = append(w.Contents, wireContent{Role: "user", Parts: []wirePart{{
				FunctionResponse: &wireFunctionResponse{Name: name, Response: map[string]any{"content": content}},
			}}})
		}
	}
	if len(system) > 0 {
		w.SystemInstruction = &wireContent{Parts: []wirePart{{Text: strings.Join(system, "\n\n")}}}
	}
	if len(req.Tools) > 0 {
		var decls []wireFunctionDecl
		for _, ts := range req.Tools {
			decls = append(decls, wireFunctionDecl{Name: ts.Name, Description: ts.Description, Parameters: ts.Parameters})
		}
		w.Tools = []wireTool{{FunctionDeclarations: decls}}
	}
	if req.MaxTokens > 0 || thinkingBudget(req.Reasoning) > 0 {
		cfg := &wireGenerationCfg{MaxOutputTokens: req.MaxTokens}
		if b := thinkingBudget(req.Reasoning); b > 0 {
			cfg.ThinkingConfig = &wireThinkingConfig{ThinkingBudget: b}
		}
		w.GenerationConfig = cfg
	}
	return w
}

func thinkingBudget(r core.ReasoningLevel) int {
	switch r {
	case core.ReasoningMinimal:
		return 512
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

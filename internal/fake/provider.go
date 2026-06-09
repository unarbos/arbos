// Package fake provides deterministic in-memory implementations of every port.
// They exist so the kernel is runnable and fully testable with zero external
// dependencies (no network, no real model, no disk). They are test doubles, not
// product adapters — real providers/tools/stores live behind the same ports.
package fake

import (
	"context"
	"encoding/json"
	"strings"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/ports"
)

// Provider is a scripted LLMProvider. Its behavior is a pure function of the
// conversation so turns replay identically:
//   - if the latest user text mentions "tool" and no tool result exists yet,
//     it requests the ls coding tool once;
//   - otherwise it streams a short canned answer.
type Provider struct{}

func (Provider) Name() string { return "fake" }

func (Provider) Capabilities() ports.Capabilities {
	return ports.Capabilities{Tools: true}
}

func (Provider) Stream(ctx context.Context, req core.LLMRequest) (<-chan core.LLMChunk, error) {
	out := make(chan core.LLMChunk, 8)
	go func() {
		defer close(out)

		var lastUser string
		hasToolResult := false
		for _, m := range req.Messages {
			switch m.Role {
			case core.RoleUser:
				lastUser = m.Content
			case core.RoleTool:
				hasToolResult = true
			default:
				// system/assistant roles are not inspected by this scripted provider
			}
		}

		if strings.Contains(strings.ToLower(lastUser), "fetch") && !hasToolResult {
			send(ctx, out, core.LLMChunk{
				ToolCalls: []core.ToolCall{{
					ID:   "call_fetch",
					Name: "fetch",
					Args: json.RawMessage(`{"url":"https://example.com"}`),
				}},
			})
			send(ctx, out, core.LLMChunk{Done: true})
			return
		}

		if strings.Contains(lastUser, "fake__ping") && !hasToolResult {
			send(ctx, out, core.LLMChunk{
				ToolCalls: []core.ToolCall{{
					ID:   "call_mcp",
					Name: "fake__ping",
					Args: json.RawMessage(`{}`),
				}},
			})
			send(ctx, out, core.LLMChunk{Done: true})
			return
		}

		if strings.Contains(lastUser, "arbos_version") && !hasToolResult {
			send(ctx, out, core.LLMChunk{
				ToolCalls: []core.ToolCall{{
					ID:   "call_ver",
					Name: "arbos_version",
					Args: json.RawMessage(`{}`),
				}},
			})
			send(ctx, out, core.LLMChunk{Done: true})
			return
		}

		if strings.Contains(strings.ToLower(lastUser), "tool") && !hasToolResult {
			send(ctx, out, core.LLMChunk{
				ToolCalls: []core.ToolCall{{
					ID:   "call_1",
					Name: "ls",
					Args: json.RawMessage(`{"path":"."}`),
				}},
			})
			send(ctx, out, core.LLMChunk{Done: true})
			return
		}

		for _, tok := range []string{"This ", "is ", "a ", "deterministic ", "fake ", "response."} {
			if !send(ctx, out, core.LLMChunk{ContentDelta: tok}) {
				return
			}
		}
		send(ctx, out, core.LLMChunk{Done: true, Usage: &core.Usage{TotalTokens: 42}})
	}()
	return out, nil
}

func send(ctx context.Context, out chan<- core.LLMChunk, c core.LLMChunk) bool {
	select {
	case <-ctx.Done():
		return false
	case out <- c:
		return true
	}
}

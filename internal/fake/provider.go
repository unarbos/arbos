// Package fake provides deterministic in-memory implementations of every port.
// They exist so the kernel is runnable and fully testable with zero external
// dependencies (no network, no real model, no disk). They are test doubles, not
// product adapters — real providers/tools/stores live behind the same ports.
package fake

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"time"

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

		if strings.Contains(lastUser, "longstream") {
			streamLong(ctx, out)
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

// streamLong emits a deterministic, markdown-heavy long reply paced like a
// fast real model (~250 tok/s). It exists for frontend streaming benchmarks:
// identical token sequence and cadence on every run, so render-performance
// deltas measured against it are attributable to the frontend alone.
func streamLong(ctx context.Context, out chan<- core.LLMChunk) {
	const sections = 40
	tick := time.NewTicker(4 * time.Millisecond)
	defer tick.Stop()
	emit := func(tok string) bool {
		select {
		case <-ctx.Done():
			return false
		case <-tick.C:
		}
		return send(ctx, out, core.LLMChunk{ContentDelta: tok})
	}
	words := func(s string) bool {
		for _, w := range strings.SplitAfter(s, " ") {
			if !emit(w) {
				return false
			}
		}
		return true
	}
	for i := 0; i < sections; i++ {
		ok := words(fmt.Sprintf("\n\n## Section %d: deterministic output\n\n", i)) &&
			words("This paragraph exists to exercise the **markdown** renderer with ") &&
			words(fmt.Sprintf("`inline code %d`, *emphasis*, and a [link](https://example.com/%d). ", i, i)) &&
			words("Streaming text accumulates one word at a time, the way a real model answers, so per-token render cost is visible.\n\n") &&
			words(fmt.Sprintf("- bullet one for section %d\n- bullet two with `code`\n- bullet three **bold**\n\n", i))
		if !ok {
			return
		}
		if i%4 == 3 {
			if !words(fmt.Sprintf("```go\nfunc section%d() error {\n\t// deterministic body %d\n\tfor i := 0; i < %d; i++ {\n\t\tfmt.Println(\"hello\", i)\n\t}\n\treturn nil\n}\n```\n\n", i, i, i)) {
				return
			}
		}
	}
	send(ctx, out, core.LLMChunk{Done: true, Usage: &core.Usage{TotalTokens: 4242}})
}

func send(ctx context.Context, out chan<- core.LLMChunk, c core.LLMChunk) bool {
	select {
	case <-ctx.Done():
		return false
	case out <- c:
		return true
	}
}

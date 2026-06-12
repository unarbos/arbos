package engine

import (
	"context"
	"strings"

	"github.com/unarbos/arbos/internal/core"
)

// streamResult is the accumulated outcome of one provider response. Keeping
// accumulation in one place means new chunk fields have a single tested home.
type streamResult struct {
	content      string
	reasoning    string
	toolCalls    []core.ToolCall
	citations    []core.Citation
	images       []core.ContentBlock
	usage        *core.Usage
	finishReason string
	err          error // mid-stream provider failure (LLMChunk.Err)
}

func (e *Engine) streamResponse(ctx context.Context, c *Conversation, chunks <-chan core.LLMChunk) streamResult {
	var content, reasoning strings.Builder
	var res streamResult
	for ch := range chunks {
		if ch.ContentDelta != "" {
			content.WriteString(ch.ContentDelta)
			if !c.emit(ctx, core.MessageDelta{Text: ch.ContentDelta}) {
				drainChunks(chunks)
				break
			}
		}
		if ch.ReasoningDelta != "" {
			reasoning.WriteString(ch.ReasoningDelta)
			if !c.emit(ctx, core.ReasoningDelta{Text: ch.ReasoningDelta}) {
				drainChunks(chunks)
				break
			}
		}
		if ch.ToolProgress != nil {
			// A tool call's arguments accumulating mid-stream: forward it as a
			// live progress event (never logged) so the frontend can show a
			// composing card in the gap before the finished call lands.
			if !c.emit(ctx, core.ToolProgress{
				CallID: ch.ToolProgress.ID,
				Name:   ch.ToolProgress.Name,
				Bytes:  ch.ToolProgress.Bytes,
			}) {
				drainChunks(chunks)
				break
			}
		}
		if len(ch.ToolCalls) > 0 {
			res.toolCalls = append(res.toolCalls, ch.ToolCalls...)
		}
		if len(ch.Citations) > 0 {
			res.citations = append(res.citations, ch.Citations...)
		}
		if len(ch.Images) > 0 {
			res.images = append(res.images, ch.Images...)
		}
		if ch.Usage != nil {
			res.usage = ch.Usage
		}
		if ch.FinishReason != "" {
			res.finishReason = ch.FinishReason
		}
		if ch.Err != nil {
			res.err = ch.Err
		}
	}
	res.content = content.String()
	res.reasoning = reasoning.String()
	return res
}

// drainChunks consumes any remaining chunks in the background so a cancelled
// turn doesn't leak a provider goroutine blocked on a send. Providers must honor
// ctx (they will stop and close), so this drains quickly; it's a safety net for
// the window between cancellation and the provider noticing.
func drainChunks(chunks <-chan core.LLMChunk) {
	go func() {
		for range chunks {
		}
	}()
}

func stopReasonFor(finishReason string) core.StopReason {
	switch finishReason {
	case "length", "max_tokens":
		return core.StopLengthLimit
	default:
		return core.StopAnswered
	}
}

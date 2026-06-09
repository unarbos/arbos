// Package tool provides the ports.ToolRuntime implementation the kernel uses:
// a registry of named tools with typed handlers and build-time-generated JSON
// schemas (ADR-0004). It is the seam the whole tool catalog plugs into; the
// engine only ever sees ports.ToolRuntime.
package tool

import (
	"context"
	"encoding/json"
	"fmt"
	"sort"
	"sync"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/ports"
)

// Result is what a tool handler produces. Content is the canonical text the
// model sees; Blocks carries non-text output (images); Details carries
// structured data the model never sees (a diff, a unified patch, a truncation
// record) for renderers and compaction. A plain-text tool returns only Content
// — NewSpec builds exactly that, so the simple path stays a one-liner.
type Result struct {
	Content   string
	Blocks    []core.ContentBlock
	Details   json.RawMessage
	Terminate bool
}

// Spec declares a tool: its identity, whether it is read-only (parallel-safe),
// a zero-value of its argument struct (for the schema generator to reflect), and
// the decode+invoke closure. Build a Spec with NewSpec (text result) or
// NewRichSpec (blocks/details result) so the arg type, the decoder, and the
// handler can never disagree.
type Spec struct {
	Name        string
	Description string
	ReadOnly    bool
	ArgSample   any
	Invoke      func(ctx context.Context, raw json.RawMessage) (Result, error)
}

// NewSpec ties a typed handler to its argument struct T. The returned Spec
// decodes raw JSON args into T before calling fn, so the handler never touches
// json.RawMessage and T is the single source of truth the generated schema is
// reflected from. fn returns plain text, wrapped into Result.Content — the
// common case for most tools.
func NewSpec[T any](name, description string, readOnly bool, fn func(ctx context.Context, args T) (string, error)) Spec {
	return newSpec(name, description, readOnly, *new(T), func(ctx context.Context, args T) (Result, error) {
		s, err := fn(ctx, args)
		return Result{Content: s}, err
	})
}

// NewRichSpec is NewSpec for tools that return multimodal content (images) or
// structured details (a diff, a truncation record) alongside their text. The
// handler returns a Result directly. Decode and schema-reflection are identical
// to NewSpec, so the two share one home and cannot drift.
func NewRichSpec[T any](name, description string, readOnly bool, fn func(ctx context.Context, args T) (Result, error)) Spec {
	return newSpec(name, description, readOnly, *new(T), fn)
}

// newSpec is the shared constructor: it owns the raw-JSON decode into T so both
// NewSpec and NewRichSpec decode identically and a handler never sees
// json.RawMessage.
func newSpec[T any](name, description string, readOnly bool, sample T, fn func(ctx context.Context, args T) (Result, error)) Spec {
	return Spec{
		Name:        name,
		Description: description,
		ReadOnly:    readOnly,
		ArgSample:   sample,
		Invoke: func(ctx context.Context, raw json.RawMessage) (Result, error) {
			var args T
			if len(raw) > 0 {
				if err := json.Unmarshal(raw, &args); err != nil {
					return Result{}, fmt.Errorf("decode args: %w", err)
				}
			}
			return fn(ctx, args)
		},
	}
}

type entry struct {
	schema core.ToolSchema
	invoke func(ctx context.Context, raw json.RawMessage) (Result, error)
}

// Registry implements ports.ToolRuntime over an in-memory map of tools.
type Registry struct {
	mu      sync.RWMutex
	order   []string
	entries map[string]entry
}

var _ ports.ToolRuntime = (*Registry)(nil)

func New() *Registry {
	return &Registry{entries: make(map[string]entry)}
}

// Register adds a tool with its generated schema. The schema's Name/ReadOnly are
// taken from the Spec so the advertised metadata matches the handler. Returns an
// error on a duplicate name — silent shadowing was a real Hermes footgun.
func (r *Registry) Register(s Spec, schema json.RawMessage) error {
	r.mu.Lock()
	defer r.mu.Unlock()
	if _, ok := r.entries[s.Name]; ok {
		return fmt.Errorf("tool %q already registered", s.Name)
	}
	r.entries[s.Name] = entry{
		schema: core.ToolSchema{
			Name:        s.Name,
			Description: s.Description,
			Parameters:  schema,
			ReadOnly:    s.ReadOnly,
		},
		invoke: s.Invoke,
	}
	r.order = append(r.order, s.Name)
	return nil
}

// Schemas returns the advertised tools in registration order.
func (r *Registry) Schemas() []core.ToolSchema {
	r.mu.RLock()
	defer r.mu.RUnlock()
	out := make([]core.ToolSchema, 0, len(r.order))
	for _, name := range r.order {
		out = append(out, r.entries[name].schema)
	}
	return out
}

// Dispatch runs a tool call. Per the port contract it never returns a Go error
// and never panics to the caller: an unknown tool, a decode failure, a handler
// error, or a handler panic all become a ToolResult with IsError set, so one bad
// tool degrades a turn instead of crashing the host.
func (r *Registry) Dispatch(ctx context.Context, call core.ToolCall) (res core.ToolResult) {
	r.mu.RLock()
	e, ok := r.entries[call.Name]
	r.mu.RUnlock()
	if !ok {
		return core.ToolResult{CallID: call.ID, IsError: true, Content: "unknown tool: " + call.Name}
	}

	defer func() {
		if rec := recover(); rec != nil {
			res = core.ToolResult{CallID: call.ID, IsError: true, Content: fmt.Sprintf("panic in tool %q: %v", call.Name, rec)}
		}
	}()

	out, err := e.invoke(ctx, call.Args)
	if err != nil {
		return core.ToolResult{CallID: call.ID, IsError: true, Content: err.Error()}
	}
	return core.ToolResult{CallID: call.ID, Content: out.Content, Blocks: out.Blocks, Details: out.Details, Terminate: out.Terminate}
}

// Names returns the registered tool names, sorted — handy for tests and the
// generator's stable output.
func (r *Registry) Names() []string {
	r.mu.RLock()
	defer r.mu.RUnlock()
	out := append([]string(nil), r.order...)
	sort.Strings(out)
	return out
}

// Package extension is arbos's Go-native equivalent of pi's self-extensible
// plugin system. pi loads TypeScript modules at runtime; a single static Go
// binary cannot, so the faithful-in-spirit seam is build-time registration: an
// Extension is a Go function that registers tools, slash commands, and event
// handlers through an API. The event bus uses pi's event names so handlers port
// across with the same vocabulary.
//
// Staged: this seam is built and tested but no entrypoint loads extensions yet
// (ADR-0030), so it is experimental until a host wires a Host into its engine.
//
// The bus is a ports.Observer: it receives every KernelEvent the engine emits
// and fans out to handlers keyed by the pi event name, so an extension observes
// agent/turn/message/tool lifecycle exactly as in pi.
package extension

import (
	"context"
	"encoding/json"
	"sync"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/ports"
	"github.com/unarbos/arbos/internal/tool"
)

// pi event names (matching pi-agent-core / coding-agent extension events).
const (
	EventAgentEnd           = "agent_end"
	EventTurnEnd            = "turn_end"
	EventMessageUpdate      = "message_update"
	EventToolExecutionStart = "tool_execution_start"
	EventToolExecutionEnd   = "tool_execution_end"
	EventToolCall           = "tool_call"
	EventToolResult         = "tool_result"
	EventError              = "error"
)

// Event is delivered to handlers: a pi event name plus the underlying arbos
// KernelEvent for handlers that want the typed payload.
type Event struct {
	Type   string
	Kernel core.KernelEvent
}

// Handler reacts to an event. It must be cheap and non-blocking (it runs on the
// engine's emit path, like any Observer).
type Handler func(ctx context.Context, ev Event)

// CommandHandler expands a slash command's arguments into prompt text (or an
// action that returns text). Matches pi's command handlers in spirit.
type CommandHandler func(args string) (string, error)

// API is what an Extension uses to register capabilities.
type API interface {
	RegisterTool(spec tool.Spec, schema json.RawMessage) error
	RegisterCommand(name, description string, h CommandHandler)
	On(event string, h Handler)
}

// Extension is a build-time plugin: a function that registers via the API.
type Extension func(API) error

// Host holds the registered tools, commands, and event bus. Tools land in the
// provided registry; the bus is wired to the engine via WithObserver.
type Host struct {
	tools    *tool.Registry
	commands map[string]registeredCommand
	bus      *Bus
}

type registeredCommand struct {
	description string
	handler     CommandHandler
}

// NewHost builds a host whose extension tools register into reg.
func NewHost(reg *tool.Registry) *Host {
	return &Host{tools: reg, commands: map[string]registeredCommand{}, bus: newBus()}
}

// Load runs each extension against the host's API, accumulating registrations.
func (h *Host) Load(exts ...Extension) error {
	for _, ext := range exts {
		if err := ext(h); err != nil {
			return err
		}
	}
	return nil
}

// Observer returns the event bus as a ports.Observer to pass to the engine.
func (h *Host) Observer() ports.Observer { return h.bus }

// Command returns a registered command handler by name.
func (h *Host) Command(name string) (CommandHandler, bool) {
	c, ok := h.commands[name]
	return c.handler, ok
}

// RegisterTool adds an extension tool to the registry.
func (h *Host) RegisterTool(spec tool.Spec, schema json.RawMessage) error {
	return h.tools.Register(spec, schema)
}

// RegisterCommand adds a slash command.
func (h *Host) RegisterCommand(name, description string, handler CommandHandler) {
	h.commands[name] = registeredCommand{description: description, handler: handler}
}

// On subscribes a handler to a pi event name.
func (h *Host) On(event string, handler Handler) { h.bus.on(event, handler) }

// Bus is the event bus: a ports.Observer that maps each KernelEvent to one or
// more pi event names and dispatches to handlers.
type Bus struct {
	mu       sync.RWMutex
	handlers map[string][]Handler
}

func newBus() *Bus { return &Bus{handlers: map[string][]Handler{}} }

func (b *Bus) on(event string, h Handler) {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.handlers[event] = append(b.handlers[event], h)
}

var _ ports.Observer = (*Bus)(nil)

// ObserveEvent maps an arbos KernelEvent to pi event names and dispatches.
func (b *Bus) ObserveEvent(ctx context.Context, ev core.KernelEvent) {
	for _, name := range piEventNames(ev) {
		b.dispatch(ctx, name, Event{Type: name, Kernel: ev})
	}
}

func (b *Bus) dispatch(ctx context.Context, name string, ev Event) {
	b.mu.RLock()
	hs := b.handlers[name]
	b.mu.RUnlock()
	for _, h := range hs {
		h(ctx, ev)
	}
}

// piEventNames maps an arbos KernelEvent to the pi event name(s) it corresponds
// to. A tool start/finish maps to both the execution-lifecycle and the
// tool_call/tool_result names pi exposes.
func piEventNames(ev core.KernelEvent) []string {
	switch ev.(type) {
	case core.MessageDelta, core.ReasoningDelta:
		return []string{EventMessageUpdate}
	case core.ToolStarted:
		return []string{EventToolExecutionStart, EventToolCall}
	case core.ToolFinished:
		return []string{EventToolExecutionEnd, EventToolResult}
	case core.TurnComplete:
		return []string{EventTurnEnd, EventAgentEnd}
	case core.ErrorEvent:
		return []string{EventError}
	default:
		return nil
	}
}

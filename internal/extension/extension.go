// Package extension is arbos's Go-native equivalent of pi's self-extensible
// plugin system. pi loads TypeScript modules at runtime; a single static Go
// binary cannot, so the faithful-in-spirit seam is build-time registration: an
// Extension is a Go function that registers tools through an API (ADR-0030).
//
// The surface is deliberately tools-only. Earlier revisions also carried an
// event bus and a slash-command registry, but no host ever consumed them; they
// were deleted rather than left as dead seams. If a host needs event handlers
// or commands later, they return as additive API methods.
package extension

import (
	"encoding/json"

	"github.com/unarbos/arbos/internal/tool"
)

// API is what an Extension uses to register capabilities.
type API interface {
	RegisterTool(spec tool.Spec, schema json.RawMessage) error
}

// Extension is a build-time plugin: a function that registers via the API.
type Extension func(API) error

// Host loads extensions; their tools land in the provided registry.
type Host struct {
	tools *tool.Registry
}

// NewHost builds a host whose extension tools register into reg.
func NewHost(reg *tool.Registry) *Host {
	return &Host{tools: reg}
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

// RegisterTool adds an extension tool to the registry.
func (h *Host) RegisterTool(spec tool.Spec, schema json.RawMessage) error {
	return h.tools.Register(spec, schema)
}

// Package pi assembles arbos's first-class in-house coding agent ("pi"): a
// coding-tuned configuration of the kernel built from the existing ports. This
// file holds the model-metadata registry: the one slice of pi-ai's per-model
// data arbos consumes (ADR-0023). Only ContextWindow is used — it drives the
// compaction trigger, which the neutral provider port does not carry. Reasoning
// support and vision are advertised by the provider's Capabilities, and the
// reasoning level is mapped per-provider inside each adapter, so neither needs a
// per-model field here.
package pi

// ModelInfo is the per-model metadata the coding agent needs. It is deliberately
// minimal: only the fields arbos actually consumes.
type ModelInfo struct {
	ID            string
	ContextWindow int // total context window in tokens; drives the compaction trigger
}

// ModelRegistry resolves a model id to its metadata, falling back to a
// conservative default for ids it does not know so an unconfigured or novel
// model degrades gracefully rather than failing.
type ModelRegistry struct {
	models   map[string]ModelInfo
	fallback ModelInfo
}

// DefaultFallback is the metadata used for an unknown model id. The context
// window is intentionally modest so compaction triggers early rather than
// letting an unknown model blow its real window.
var DefaultFallback = ModelInfo{
	ContextWindow: 128_000,
}

// NewModelRegistry builds a registry from a fallback and zero or more entries
// keyed by ModelInfo.ID.
func NewModelRegistry(fallback ModelInfo, models ...ModelInfo) *ModelRegistry {
	m := make(map[string]ModelInfo, len(models))
	for _, mi := range models {
		m[mi.ID] = mi
	}
	return &ModelRegistry{models: m, fallback: fallback}
}

// DefaultModelRegistry is the seed used until the assembly phase wires the
// concrete models in use. It carries only the conservative fallback; concrete
// entries are added by the caller via NewModelRegistry.
func DefaultModelRegistry() *ModelRegistry {
	return NewModelRegistry(DefaultFallback)
}

// SeededModelRegistry carries the published context windows for the commonly
// used models across the native providers (OpenAI-compatible, Anthropic,
// Google), so the compaction trigger fires at each model's real budget instead
// of the conservative fallback. Unknown ids still fall back to DefaultFallback.
// Extend as new models ship.
func SeededModelRegistry() *ModelRegistry {
	return NewModelRegistry(DefaultFallback,
		// Anthropic
		ModelInfo{ID: "claude-sonnet-4-20250514", ContextWindow: 200_000},
		ModelInfo{ID: "claude-opus-4-20250514", ContextWindow: 200_000},
		ModelInfo{ID: "claude-3-5-haiku-20241022", ContextWindow: 200_000},
		// Anthropic via OpenRouter (slug form)
		ModelInfo{ID: "anthropic/claude-opus-4.8", ContextWindow: 1_000_000},
		ModelInfo{ID: "anthropic/claude-opus-4.8-fast", ContextWindow: 1_000_000},
		// Google Gemini
		ModelInfo{ID: "gemini-2.5-pro", ContextWindow: 1_048_576},
		ModelInfo{ID: "gemini-2.5-flash", ContextWindow: 1_048_576},
		// OpenAI-compatible
		ModelInfo{ID: "gpt-5", ContextWindow: 400_000},
		ModelInfo{ID: "gpt-5.5", ContextWindow: 400_000},
		ModelInfo{ID: "gpt-4o", ContextWindow: 128_000},
		// gm (saygm.com) frontier ids — bare, no provider prefix (ADR-0040)
		ModelInfo{ID: "claude-opus-4-8", ContextWindow: 1_000_000},
	)
}

// Get returns the metadata for an id, or the fallback (stamped with the id) for
// an unknown one, so callers never have to nil-check.
func (r *ModelRegistry) Get(id string) ModelInfo {
	if mi, ok := r.models[id]; ok {
		return mi
	}
	mi := r.fallback
	mi.ID = id
	return mi
}

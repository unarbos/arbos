package mind

import (
	"context"
	"fmt"
	"reflect"
	"strings"

	"github.com/unarbos/arbos/internal/tool"
	"github.com/unarbos/arbos/internal/tool/codingspec"
	"github.com/unarbos/arbos/internal/tool/jsonschema"
)

// Writer is the narrow capability the remember tool needs: persist one atom.
// *Mind satisfies it (Remember), so the tool stays decoupled from the store.
type Writer interface {
	Remember(ctx context.Context, content string) error
}

// RememberArgs are the arguments to the remember tool.
type RememberArgs struct {
	Fact string `json:"fact" desc:"A durable fact worth keeping across all future sessions — a preference, a decision, a learned property of the world or the codebase. Not transient task data (that belongs in a plan node's outcome)."`
}

// RegisterRememberTool adds the remember tool to a registry — the explicit
// write half of memory, the symmetric counterpart of the recall that is
// injected at turn start. Like delegate/plan/notify it is reflected at
// registration (the documented ADR-0004 carve-out for dynamically wired
// tools).
func RegisterRememberTool(reg *tool.Registry, w Writer) error {
	schema, err := jsonschema.Reflect(reflect.TypeOf(RememberArgs{}))
	if err != nil {
		return fmt.Errorf("remember schema: %w", err)
	}
	spec := tool.NewSpec("remember",
		"Persist a durable fact to your long-term memory so it survives this session and surfaces in future ones. Use it the moment you learn something worth keeping — a user preference or correction, a decision, a stable property of the codebase. It writes immediately; do not rely on background distillation for things you were explicitly told to remember.",
		false,
		func(ctx context.Context, a RememberArgs) (string, error) {
			fact := strings.TrimSpace(a.Fact)
			if fact == "" {
				return "", fmt.Errorf("remember: fact must not be empty")
			}
			if err := w.Remember(ctx, fact); err != nil {
				return "", err
			}
			return "Remembered.", nil
		})
	return reg.Register(spec, schema)
}

// RememberPromptInfo is the remember tool's system-prompt metadata.
func RememberPromptInfo() codingspec.ToolPromptInfo {
	return codingspec.ToolPromptInfo{
		Name:    "remember",
		Snippet: "Persist a durable fact to long-term memory (survives this session; recalled in future ones)",
		Guidelines: []string{
			"When the user tells you to remember something, or corrects a fact about themselves or the world, or you learn a durable property of the codebase, call remember immediately — it writes now and is recalled in future sessions. Never claim to have remembered something without calling it.",
			"Memory has tiers: durable knowledge goes to remember; transient task results (a price you just fetched, a build's output) go in a plan node's outcome; a message the user must see goes through notify. Keep ephemeral data out of remember.",
		},
	}
}

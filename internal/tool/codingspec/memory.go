package codingspec

import (
	"context"
	"fmt"
	"strings"

	"github.com/unarbos/arbos/internal/memory"
	"github.com/unarbos/arbos/internal/tool"
)

// MemoryArgs are the arguments to memory.
type MemoryArgs struct {
	Action string `json:"action" desc:"One of: remember, recall, forget, list."`
	Key    string `json:"key,omitempty" desc:"Key for remember, recall, or forget."`
	Value  string `json:"value,omitempty" desc:"Value for remember."`
}

func memorySpec(store *memory.Store) tool.Spec {
	return tool.NewSpec("memory",
		"Store and recall persistent facts across sessions. Project-scoped entries live in .arbos/memory/; user-scoped entries in ~/.config/arbos/memory/.",
		false,
		func(_ context.Context, a MemoryArgs) (string, error) {
			if store == nil {
				return "", fmt.Errorf("memory: store not configured")
			}
			switch strings.ToLower(strings.TrimSpace(a.Action)) {
			case "remember":
				if err := store.Remember(a.Key, a.Value); err != nil {
					return "", err
				}
				return fmt.Sprintf("Remembered %q.", strings.TrimSpace(a.Key)), nil
			case "recall":
				v, ok, err := store.Recall(a.Key)
				if err != nil {
					return "", err
				}
				if !ok {
					return fmt.Sprintf("No memory entry for key %q.", strings.TrimSpace(a.Key)), nil
				}
				return v, nil
			case "forget":
				if err := store.Forget(a.Key); err != nil {
					return "", err
				}
				return fmt.Sprintf("Forgot %q.", strings.TrimSpace(a.Key)), nil
			case "list":
				keys, err := store.List()
				if err != nil {
					return "", err
				}
				if len(keys) == 0 {
					return "No memory entries.", nil
				}
				return strings.Join(keys, "\n"), nil
			default:
				return "", fmt.Errorf("memory: unknown action %q (use remember, recall, forget, or list)", a.Action)
			}
		})
}

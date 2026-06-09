package codingspec

import (
	"context"
	"encoding/json"
	"fmt"
	"os"

	"github.com/unarbos/arbos/internal/tool"
)

// EditArgs are the arguments to edit.
type EditArgs struct {
	Path  string `json:"path" desc:"Path to the file to edit, relative to the workspace root."`
	Edits []Edit `json:"edits" desc:"One or more targeted replacements. Each edit is matched against the original file, not incrementally. Do not include overlapping or nested edits. If two changes touch the same block or nearby lines, merge them into one edit instead."`
}

// editDetails is the model-invisible structured payload consumed by the TUI: a
// line-numbered display diff plus the replaced-block count and first changed line
// (used for navigation). The model still only sees the plain success message.
type editDetails struct {
	ReplacedBlocks   int    `json:"replacedBlocks"`
	FirstChangedLine int    `json:"firstChangedLine"`
	Diff             string `json:"diff,omitempty"`
}

// editSpec edits a file with one or more exact-text replacements, faithful to
// pi's edit tool (editdiff.go holds the matcher). It preserves BOM and line
// endings and returns pi's exact success message; failures surface pi's verbatim
// error strings.
func editSpec(root string) tool.Spec {
	return tool.NewRichSpec("edit",
		"Edit a single file using exact text replacement. Every edits[].oldText must match a unique, non-overlapping region of the original file. If two changes affect the same block or nearby lines, merge them into one edit instead of emitting overlapping edits. Do not include large unchanged regions just to connect distant changes.",
		false,
		func(_ context.Context, a EditArgs) (tool.Result, error) {
			if len(a.Edits) == 0 {
				return tool.Result{}, fmt.Errorf("Edit tool input is invalid. edits must contain at least one replacement.")
			}
			abs, err := tool.Resolve(root, a.Path)
			if err != nil {
				return tool.Result{}, err
			}
			raw, err := os.ReadFile(abs)
			if err != nil {
				return tool.Result{}, fmt.Errorf("Could not edit file: %s. %s.", a.Path, err)
			}

			bom, content := stripBom(string(raw))
			ending := detectLineEnding(content)
			normalized := normalizeToLF(content)
			base, newContent, err := applyEditsToNormalizedContent(normalized, a.Edits, a.Path)
			if err != nil {
				return tool.Result{}, err
			}

			final := bom + restoreLineEndings(newContent, ending)
			if err := os.WriteFile(abs, []byte(final), 0o644); err != nil {
				return tool.Result{}, fmt.Errorf("Could not edit file: %s. %s.", a.Path, err)
			}

			details, _ := json.Marshal(editDetails{
				ReplacedBlocks:   len(a.Edits),
				FirstChangedLine: firstChangedLine(base, newContent),
				Diff:             generateDiffString(base, newContent, 3),
			})
			return tool.Result{
				Content: fmt.Sprintf("Successfully replaced %d block(s) in %s.", len(a.Edits), a.Path),
				Details: details,
			}, nil
		})
}

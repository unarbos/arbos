package codingspec

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"strings"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/tool"
)

// EditArgs are the arguments to edit.
type EditArgs struct {
	Path  string `json:"path" desc:"Path to the file to edit, relative to the workspace root."`
	Edits []Edit `json:"edits" desc:"One or more targeted replacements. Each edit is matched against the original file, not incrementally. Do not include overlapping or nested edits. If two changes touch the same block or nearby lines, merge them into one edit instead."`
}

// editDetails is the model-invisible structured payload consumed by the TUI: a
// line-numbered display diff plus the replaced-block count and first changed line
// (used for navigation). The model sees the success message and updated-region
// snippet, not this diff.
type editDetails struct {
	ReplacedBlocks   int    `json:"replacedBlocks"`
	FirstChangedLine int    `json:"firstChangedLine"`
	Diff             string `json:"diff,omitempty"`
}

// editSnippetContext is the unchanged lines shown around each updated region.
const editSnippetContext = 2

// editSnippetMaxLines caps the total updated-region snippet so a sweeping
// replaceAll cannot flood the context window.
const editSnippetMaxLines = 60

// editSpec edits a file with one or more targeted replacements (editdiff.go
// holds the matcher and its tier semantics). It preserves BOM and line endings.
// Robustness around the matcher, both deliberate divergences from pi:
//
//   - Staleness guard: if the read ledger knows what version of the file the
//     model last saw this turn and the file on disk differs (a background job
//     or another actor wrote it), the edit is refused instead of applied
//     against content the model never read.
//   - Verification loop: success returns the updated regions with their NEW
//     line numbers, and records them in the ledger, so the model can confirm
//     placement and keep editing without a re-read.
func editSpec(root string, ledger *readLedger) tool.Spec {
	spec := tool.NewRichSpec("edit",
		"Edit a single file using targeted text replacement. Matching tries exact text first, then tolerates indentation shifts and unicode punctuation differences over whole lines. Each edits[].oldText must match exactly one region of the original file unless its replaceAll is set. Matched regions must not overlap; if two changes affect the same block or nearby lines, merge them into one edit. Do not include large unchanged regions just to connect distant changes.",
		false,
		func(ctx context.Context, a EditArgs) (tool.Result, error) {
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

			turn := turnFromContext(ctx)
			if prev, ok := ledger.peekVersion(turn, abs); ok && prev != fileVersion(raw) {
				return tool.Result{}, fmt.Errorf("%s changed on disk since you last read it this turn, so the content you were shown is stale. Re-read the file and retry the edit.", a.Path)
			}

			bom, content := stripBom(string(raw))
			ending := detectLineEnding(content)
			normalized := normalizeToLF(content)
			newContent, applied, err := applyEdits(normalized, a.Edits, a.Path)
			if err != nil {
				return tool.Result{}, err
			}

			final := bom + restoreLineEndings(newContent, ending)
			if err := os.WriteFile(abs, []byte(final), 0o644); err != nil {
				return tool.Result{}, fmt.Errorf("Could not edit file: %s. %s.", a.Path, err)
			}

			ranges := changedLineRanges(normalized, applied)
			snippet, shown := renderUpdatedRegions(newContent, ranges)
			version := fileVersion([]byte(final))
			ledger.noteVersion(turn, abs, version)
			for _, r := range shown {
				ledger.reconcile(turn, abs, version, r.start, r.end)
			}

			first := 0
			if len(ranges) > 0 {
				first = ranges[0].start
			}
			details, _ := json.Marshal(editDetails{
				ReplacedBlocks:   len(applied),
				FirstChangedLine: first,
				Diff:             generateDiffString(normalized, newContent, 3),
			})
			msg := fmt.Sprintf("Successfully replaced %d block(s) in %s.", len(applied), a.Path)
			if snippet != "" {
				msg += "\n\nUpdated regions (new line numbers):\n" + snippet
			}
			return tool.Result{Content: msg, Details: details}, nil
		})
	return tool.WithAccess(spec, func(a EditArgs) core.AccessSet {
		return writeAccess(root, a.Path)
	})
}

// renderUpdatedRegions renders the changed line ranges of the new content with
// line numbers and editSnippetContext lines of context, merging overlapping
// regions and capping the total at editSnippetMaxLines. It returns the snippet
// and the exact ranges shown, which the caller records in the read ledger.
func renderUpdatedRegions(newContent string, ranges []lineRange) (string, []lineRange) {
	lines := splitLF(newContent)
	total := len(lines)
	var merged []lineRange
	for _, r := range ranges {
		s, e := r.start-editSnippetContext, r.end+editSnippetContext
		if s < 1 {
			s = 1
		}
		if e > total {
			e = total
		}
		if e < s {
			continue
		}
		merged = mergeRange(merged, lineRange{s, e})
	}
	var parts []string
	var shown []lineRange
	budget := editSnippetMaxLines
	for _, r := range merged {
		if budget <= 0 {
			parts = append(parts, "[remaining updated regions omitted]")
			break
		}
		if n := r.end - r.start + 1; n > budget {
			r.end = r.start + budget - 1
		}
		budget -= r.end - r.start + 1
		seg := strings.Join(lines[r.start-1:r.end], "\n")
		parts = append(parts, numberLines(seg, r.start))
		shown = append(shown, r)
	}
	return strings.Join(parts, "\n...\n"), shown
}

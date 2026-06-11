package codingspec

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/unarbos/arbos/internal/tool"
)

// ShowArgs are the arguments to show.
type ShowArgs struct {
	Path  string `json:"path" desc:"Path to the file to present — relative to the working directory, or absolute."`
	Title string `json:"title,omitempty" desc:"Optional panel title shown on the tab; defaults to the file name."`
}

// surfaceDetails is show's renderer-facing payload (ToolResult.Details, the
// same channel delegate uses for childSession): a typed reference the web UI
// opens as a panel beside the chat. The model never sees it — the panel
// fetches the live file back through the gateway, so the reference stays
// small and the content stays fresh.
type surfaceDetails struct {
	Surface surface `json:"surface"`
}

type surface struct {
	Kind  string `json:"kind"`            // canvas | image | doc | code
	Path  string `json:"path"`            // workspace-relative when under root, else absolute
	Title string `json:"title,omitempty"` // panel/tab label
}

// surfaceKind classifies a file by extension into the viewer the UI picks:
// HTML renders in a sandboxed iframe (a canvas), markdown as a document,
// images directly, and everything else as code.
func surfaceKind(path string) string {
	switch strings.ToLower(filepath.Ext(path)) {
	case ".html", ".htm":
		return "canvas"
	case ".md", ".markdown":
		return "doc"
	case ".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg":
		return "image"
	default:
		return "code"
	}
}

func showSpec(root string) tool.Spec {
	return tool.NewRichSpec("show",
		"Present a file to the user in a panel beside the chat: an HTML canvas, an image, a markdown document, or a code file. Use it after producing a visual artifact instead of telling the user to open the file themselves. HTML canvases are themed by the panel: style them with the arbos `--color-*` design tokens (var(--color-token, fallback)), never a hardcoded palette.",
		true,
		func(_ context.Context, a ShowArgs) (tool.Result, error) {
			if strings.TrimSpace(a.Path) == "" {
				return tool.Result{}, fmt.Errorf("show: path is required")
			}
			abs, err := tool.Resolve(root, a.Path)
			if err != nil {
				return tool.Result{}, fmt.Errorf("show: %w", err)
			}
			info, err := os.Stat(abs)
			if err != nil {
				return tool.Result{}, fmt.Errorf("show: %w", err)
			}
			if info.IsDir() {
				return tool.Result{}, fmt.Errorf("show: %s is a directory; present a file", a.Path)
			}
			// The panel fetches the file back through the gateway, which
			// resolves against the same root — keep the reference
			// workspace-relative when the file lives under it so the round
			// trip is clean (and the chip reads naturally).
			shown := abs
			if absRoot, err := filepath.Abs(rootOrCwd(root)); err == nil {
				if rel, err := filepath.Rel(absRoot, abs); err == nil && !strings.HasPrefix(rel, "..") {
					shown = rel
				}
			}
			title := strings.TrimSpace(a.Title)
			if title == "" {
				title = filepath.Base(abs)
			}
			kind := surfaceKind(abs)
			details, err := json.Marshal(surfaceDetails{Surface: surface{
				Kind:  kind,
				Path:  filepath.ToSlash(shown),
				Title: title,
			}})
			if err != nil {
				return tool.Result{}, fmt.Errorf("show: %w", err)
			}
			return tool.Result{
				Content: fmt.Sprintf("Presented %s to the user in a panel beside the chat (%s).", shown, kind),
				Details: details,
			}, nil
		})
}

// rootOrCwd mirrors tool.Resolve's empty-base rule for the relativize step.
func rootOrCwd(root string) string {
	if root == "" {
		return "."
	}
	return root
}

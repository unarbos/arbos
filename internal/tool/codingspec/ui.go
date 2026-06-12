package codingspec

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"

	"github.com/unarbos/arbos/internal/tool"
)

// UIArgs are the arguments to ui.
type UIArgs struct {
	Action string `json:"action" desc:"open, close, or focus."`
	Target string `json:"target,omitempty" desc:"For close/focus — which side panel: 'all' to close every side panel, or text matching a panel's title or file path (e.g. 'Browser', a filename)."`
	Panel  string `json:"panel,omitempty" desc:"For open — which panel: 'chat' (a fresh conversation), 'terminal' (a fresh interactive shell), 'browser' (the live browser), 'files', 'activity', 'history', or 'settings'."`
	Cwd    string `json:"cwd,omitempty" desc:"For open terminal — the directory the shell starts in; defaults to the working directory."`
}

// uiDetails is ui's renderer-facing payload (ToolResult.Details, the same
// channel show and the browser live panel use): a command the web UI executes
// against its own layout. The model never sees it — like a surface reference,
// it rides Details and the frontend acts on tool_finished.
type uiDetails struct {
	UI uiCommand `json:"ui"`
}

type uiCommand struct {
	Action string `json:"action"`           // open | close | focus
	Target string `json:"target,omitempty"` // close/focus: "all" or a title/path substring
	Panel  string `json:"panel,omitempty"`  // open: chat | terminal | browser | files | activity | history | settings
	Cwd    string `json:"cwd,omitempty"`    // open terminal: starting directory
}

// openablePanels are the panels the open action can create. Chat starts a
// fresh conversation, terminal spawns a fresh interactive shell, browser opens
// the live browser tab; the rest are the app's singleton views.
var openablePanels = map[string]bool{
	"chat":     true,
	"terminal": true,
	"browser":  true,
	"files":    true,
	"activity": true,
	"history":  true,
	"settings": true,
}

func uiSpec() tool.Spec {
	return tool.NewRichSpec("ui",
		"Control the arbos interface beside the chat. action 'open' opens a panel: 'chat' starts a fresh conversation tab, 'terminal' spawns a fresh interactive shell for the user (optionally in cwd), 'browser' opens the live browser tab, 'files' the workspace file browser, and 'activity', 'history', 'settings' the app's views. action 'close' with target 'all' closes every side panel; target text closes the panel(s) whose title or file path matches (e.g. 'Browser'). action 'focus' raises the matching panel to the front. Use it to open what the user asks for, clean up after presenting things, or bring a panel back into view.",
		false,
		func(_ context.Context, a UIArgs) (tool.Result, error) {
			action := strings.ToLower(strings.TrimSpace(a.Action))
			switch action {
			case "open":
				panel := strings.ToLower(strings.TrimSpace(a.Panel))
				if !openablePanels[panel] {
					return tool.Result{}, fmt.Errorf("ui: open needs panel 'chat', 'terminal', 'browser', 'files', 'activity', 'history', or 'settings' (got %q)", a.Panel)
				}
				cwd := strings.TrimSpace(a.Cwd)
				if cwd != "" && panel != "terminal" {
					return tool.Result{}, fmt.Errorf("ui: cwd only applies to panel 'terminal'")
				}
				details, err := json.Marshal(uiDetails{UI: uiCommand{Action: action, Panel: panel, Cwd: cwd}})
				if err != nil {
					return tool.Result{}, fmt.Errorf("ui: %w", err)
				}
				var msg string
				switch panel {
				case "terminal":
					msg = "Opened a terminal panel with a fresh shell beside the chat."
					if cwd != "" {
						msg = fmt.Sprintf("Opened a terminal panel with a fresh shell in %s beside the chat.", cwd)
					}
				case "chat":
					msg = "Opened a fresh chat tab."
				default:
					msg = fmt.Sprintf("Opened the %s panel.", panel)
				}
				return tool.Result{Content: msg, Details: details}, nil
			case "close", "focus":
				target := strings.TrimSpace(a.Target)
				if target == "" {
					return tool.Result{}, fmt.Errorf("ui: target is required ('all' or text matching a panel)")
				}
				details, err := json.Marshal(uiDetails{UI: uiCommand{Action: action, Target: target}})
				if err != nil {
					return tool.Result{}, fmt.Errorf("ui: %w", err)
				}
				var msg string
				switch {
				case action == "close" && target == "all":
					msg = "Closed all side panels."
				case action == "close":
					msg = fmt.Sprintf("Closed side panel(s) matching %q.", target)
				default:
					msg = fmt.Sprintf("Focused the panel matching %q.", target)
				}
				return tool.Result{Content: msg, Details: details}, nil
			default:
				return tool.Result{}, fmt.Errorf("ui: action must be 'open', 'close', or 'focus' (got %q)", a.Action)
			}
		})
}

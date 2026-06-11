package main

import (
	"os"
	"os/exec"
	"path/filepath"
	"runtime/debug"
	"strings"
)

// tuiMeta carries the static context the Cursor-style chrome displays:
// version, model, working directory, and git branch.
type tuiMeta struct {
	Version string
	Model   string
	CWD     string
	Branch  string
	Approve bool
}

func newTUIMeta(model string, approve bool) tuiMeta {
	cwd, _ := os.Getwd()
	return tuiMeta{
		Version: buildVersion(),
		Model:   formatModelName(model),
		CWD:     shortenHome(cwd),
		Branch:  gitBranch(),
		Approve: approve,
	}
}

func buildVersion() string {
	if info, ok := debug.ReadBuildInfo(); ok && info.Main.Version != "" && info.Main.Version != "(devel)" {
		v := info.Main.Version
		if !strings.HasPrefix(v, "v") {
			v = "v" + v
		}
		return v
	}
	return "v0.1.0"
}

func formatModelName(id string) string {
	if id == "" || id == "fake" {
		return "Fake (offline)"
	}
	// Humanize common slugs the way Cursor does: "GPT-5.4 Mini Medium".
	repl := strings.NewReplacer(
		"anthropic/", "",
		"openai/", "",
		"google/", "",
		"-", " ",
		"/", " ",
		"_", " ",
	)
	name := repl.Replace(id)
	words := strings.Fields(name)
	for i, w := range words {
		if len(w) > 0 {
			words[i] = strings.ToUpper(w[:1]) + w[1:]
		}
	}
	return strings.Join(words, " ")
}

func shortenHome(path string) string {
	home, err := os.UserHomeDir()
	if err != nil || home == "" {
		return path
	}
	if path == home {
		return "~"
	}
	if rest, ok := strings.CutPrefix(path, home+string(filepath.Separator)); ok {
		return "~/" + rest
	}
	return path
}

func gitBranch() string {
	out, err := exec.Command("git", "rev-parse", "--abbrev-ref", "HEAD").Output()
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(out))
}

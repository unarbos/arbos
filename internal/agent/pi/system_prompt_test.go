package pi

import (
	"strings"
	"testing"
)

// TestSystemPromptCarriesGitAuthorRule guards the always-on guideline that
// stops the agent from silently committing under an inherited git identity. The
// rule must live in the assembled prompt (so every arbos instance carries it),
// not in per-session memory.
func TestSystemPromptCarriesGitAuthorRule(t *testing.T) {
	prompt := BuildSystemPrompt(PromptOptions{Cwd: "/tmp/x"})

	// Key phrases that capture the rule's intent; if the wording is revised,
	// keep the meaning these assert.
	for _, want := range []string{
		"git commit",
		"do not silently attribute it to an identity you are not sure of",
		"ask who the commit author should be",
		"Never attribute a commit to a person you are not sure is the one driving this session",
	} {
		if !strings.Contains(prompt, want) {
			t.Errorf("system prompt is missing the git-author rule fragment: %q", want)
		}
	}
}

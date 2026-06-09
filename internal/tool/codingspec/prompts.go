package codingspec

// ToolPromptInfo is the system-prompt metadata for a coding tool: the one-line
// snippet for the "Available tools" section and the guideline bullets appended
// to the prompt's Guidelines section. It lives here, with the tools, so the
// pi system-prompt assembler reads it without re-stating tool knowledge. The
// text is reproduced verbatim from pi's tool definitions.
type ToolPromptInfo struct {
	Name       string
	Snippet    string
	Guidelines []string
}

// PromptInfos returns the per-tool prompt metadata for the coding toolset, in
// the same order as Specs.
func PromptInfos() []ToolPromptInfo {
	return []ToolPromptInfo{
		{Name: "ls", Snippet: "List directory contents"},
		{Name: "read", Snippet: "Read file contents", Guidelines: []string{
			"Use read to examine files instead of cat or sed.",
		}},
		{Name: "find", Snippet: "Find files by glob pattern (respects .gitignore)"},
		{Name: "grep", Snippet: "Search file contents for patterns (respects .gitignore)"},
		{Name: "write", Snippet: "Create or overwrite files", Guidelines: []string{
			"Use write only for new files or complete rewrites.",
		}},
		{Name: "edit", Snippet: "Make precise file edits with exact text replacement, including multiple disjoint edits in one call", Guidelines: []string{
			"Use edit for precise changes (edits[].oldText must match exactly)",
			"When changing multiple separate locations in one file, use one edit call with multiple entries in edits[] instead of multiple edit calls",
			"Each edits[].oldText is matched against the original file, not after earlier edits are applied. Do not emit overlapping or nested edits. Merge nearby changes into one edit.",
			"Keep edits[].oldText as small as possible while still being unique in the file. Do not pad with large unchanged regions.",
		}},
		{Name: "bash", Snippet: "Execute bash commands (ls, grep, find, etc.)"},
		{Name: "fetch", Snippet: "Fetch a URL and return the response body as text", Guidelines: []string{
			"Use fetch for HTTP/HTTPS requests instead of curl in bash when possible.",
		}},
		{Name: "memory", Snippet: "Store and recall persistent facts across sessions", Guidelines: []string{
			"Use memory remember/recall for user preferences and durable facts.",
		}},
	}
}

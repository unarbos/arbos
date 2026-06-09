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
		{Name: "ls", Snippet: "List directory contents", Guidelines: []string{
			"Use ls for a quick name-only scan; use bash `ls -la` when the user wants detail (permissions, sizes, dates).",
		}},
		{Name: "read", Snippet: "Read file contents", Guidelines: []string{
			"Use read to examine files instead of cat or sed.",
			"read prefixes each line with its line number as `<n>|`. That prefix is display metadata: never include it in edit oldText or when quoting file contents — use the numbers only to choose offsets and to locate edits.",
		}},
		{Name: "find", Snippet: "Find files by glob pattern (respects .gitignore)"},
		{Name: "grep", Snippet: "Search file contents for patterns (respects .gitignore)"},
		{Name: "write", Snippet: "Create or overwrite files", Guidelines: []string{
			"Use write only for new files or complete rewrites.",
		}},
		{Name: "edit", Snippet: "Make precise file edits with targeted text replacement, including multiple disjoint edits in one call", Guidelines: []string{
			"Use edit for precise changes. Matching tries exact text first, then tolerates indentation shifts and unicode punctuation differences when oldText covers whole lines.",
			"When changing multiple separate locations in one file, use one edit call with multiple entries in edits[] instead of multiple edit calls",
			"Each edits[].oldText is matched against the original file, not after earlier edits are applied. Do not emit overlapping or nested edits. Merge nearby changes into one edit.",
			"Keep edits[].oldText as small as possible while still being unique in the file. Do not pad with large unchanged regions.",
			"Set replaceAll: true on an edit to change every occurrence of its oldText (renames, repeated strings); without it the match must be unique.",
			"A successful edit returns the updated regions with their new line numbers — verify placement from that output instead of re-reading the file.",
		}},
		{Name: "bash", Snippet: "Execute any shell command — file ops, network tools (ssh, scp, curl), git, package managers, scripts; long commands continue as background jobs", Guidelines: []string{
			"Prefer bash for detailed directory listings (`ls -la`), disk usage, and multi-step shell exploration.",
			"bash is a real shell with full host access: it runs network commands like ssh, scp, curl, git, and package managers, not just file operations. Never claim you lack network access or that you cannot run a command (ssh, etc.) — run it with bash.",
			"bash has no interactive stdin. For remote work, pass the command to ssh non-interactively (e.g. `ssh host \"uname -a\"`) instead of opening a bare login shell that just connects and exits.",
			"A bash command that outlives its wait window keeps running as a background job — never rerun it; follow it with await (use a regex pattern for a ready/error line) or check jobs. Background jobs survive arbos restarts.",
			"Start long-lived processes (dev servers, watchers) with background:true, then await a readiness pattern instead of sleeping. Set bash's wait higher than a command's expected runtime when you want to block on it; do useful work between awaits instead of polling idly.",
			"Never hold a turn open with sleep+await to reach a moment in time: one-shot timed work (\"in 30 minutes…\") belongs in the plan, which fires on the host clock. But a self-contained recurring loop (`while sleep 30; do …; done`) IS a legitimate background job when the user wants a mechanical command on a cadence finer than the plan clock (~1 minute): start it with background:true and end the turn — it survives restarts and costs no model turns. Use a maintain plan node instead when each recurrence needs judgment, not just a command.",
			"After any tool call, base your reply on its actual output. Never deny doing something you just did, and never tell the user to run a command you can run yourself — you already have the bash tool, so run it.",
		}},
		{Name: "await", Snippet: "Wait on a background job: returns its new output when it exits, a regex matches, or the timeout elapses"},
		{Name: "jobs", Snippet: "List background jobs (id, status, runtime, command, log path)"},
		{Name: "fetch", Snippet: "Fetch a URL and return the response body as text", Guidelines: []string{
			"Use fetch for HTTP/HTTPS requests instead of curl in bash when possible.",
		}},
	}
}

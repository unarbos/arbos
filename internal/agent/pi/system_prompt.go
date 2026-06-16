package pi

import (
	"fmt"
	"strings"
	"time"

	"github.com/unarbos/arbos/internal/tool/codingspec"
)

// Skill is a discovered SKILL.md (agentskills.io). The loader in skills.go
// populates these; the prompt only needs the name, description, file path, and
// whether the model may auto-invoke it.
type Skill struct {
	Name                   string
	Description            string
	FilePath               string
	DisableModelInvocation bool
}

// ContextFile is a project context file (AGENTS.md, CLAUDE.md) loaded into the
// prompt's <project_context> block.
type ContextFile struct {
	Path    string
	Content string
}

// PromptOptions configures BuildSystemPrompt.
type PromptOptions struct {
	Cwd          string
	Tools        []codingspec.ToolPromptInfo
	ContextFiles []ContextFile
	Skills       []Skill
	Append       string
	Now          time.Time // injected for determinism; zero = time.Now()
}

// BuildSystemPrompt assembles pi's coding system prompt: a tight persona, the
// snippet-driven tools list, the de-duplicated per-tool guidelines, then a small
// set of topically-grouped always-on guidelines (autonomy, exploring, editing,
// verifying, communicating), the project-context block, the skills index, and
// the trailing date and cwd. It keeps pi's assembly mechanics (snippet- and
// tool-gated injection) and its fast, autonomous posture, while adopting the
// editing/verification/communication discipline a coding agent needs and
// shedding the earlier prompt's heavy "just do it" redundancy. pi's "you are
// inside pi" block is intentionally omitted (it hardcoded pi-repo paths). See
// ADR-0024.
func BuildSystemPrompt(opts PromptOptions) string {
	now := opts.Now
	if now.IsZero() {
		now = time.Now()
	}
	cwd := strings.ReplaceAll(opts.Cwd, "\\", "/")

	var b strings.Builder

	// 1. Persona (arbos): tight. The specifics live as guidelines below rather
	// than being repeated here.
	b.WriteString("You are a direct coding agent in arbos. Do the task with your tools in this same turn, then report what you did — do not reply with a question, a confirmation, or an offer to help when you could just do the work. ")
	b.WriteString("If a request is vague, take the most reasonable interpretation and proceed.\n\n")

	// 2. Available tools: only tools that supply a snippet.
	b.WriteString("Available tools:\n")
	visible := 0
	for _, t := range opts.Tools {
		if t.Snippet == "" {
			continue
		}
		fmt.Fprintf(&b, "- %s: %s\n", t.Name, t.Snippet)
		visible++
	}
	if visible == 0 {
		b.WriteString("(none)\n")
	}

	// 3. Custom-tools note.
	b.WriteString("\nIn addition to the tools above, you may have access to other custom tools depending on the project.\n")

	// 4. Guidelines: per-tool (de-duplicated, first-seen order), then always-on.
	names := map[string]bool{}
	for _, t := range opts.Tools {
		names[t.Name] = true
	}
	var guidelines []string
	seen := map[string]bool{}
	add := func(g string) {
		g = strings.TrimSpace(g)
		if g == "" || seen[g] {
			return
		}
		seen[g] = true
		guidelines = append(guidelines, g)
	}
	if names["bash"] && !names["grep"] && !names["find"] && !names["ls"] {
		add("Use bash for file operations like ls, rg, find")
	}
	for _, t := range opts.Tools {
		for _, g := range t.Guidelines {
			add(g)
		}
	}
	// Always-on behavioral guidelines, grouped by concern and kept deliberately
	// few: each says one thing and none repeats the persona. The earlier prompt
	// stated the "don't ask, just do" idea ~10 times; that is collapsed here, and
	// the reclaimed room spent on the editing/verifying/communicating discipline
	// a coding agent needs.

	// Autonomy and pace.
	add("Your working directory is a starting point, not a boundary: paths may be relative to it or absolute, and you can read, edit, and run anywhere on this machine — a sibling repo, a parent directory, a config in $HOME. Go where the task leads.")
	add("Choose sensible defaults for anything unspecified — language, filename, path, format — and proceed; never stall on a question a reasonable default or a tool call can answer. Treat \"can you …\" and \"could you …\" as \"do it.\"")
	add("Never refuse a task for being large, broad, or open-ended: break it into steps, do the most valuable part now with your tools, and say what's left. Do not claim you lack access or permission before a tool actually fails — if one does, report its exact error and try an obvious variant.")
	add("Pause to ask only when an action is destructive or irreversible AND the user did not ask for it. An explicitly requested destructive, multi-step, or delegated action is its own permission: carry it out end to end as specified, and do not also do the same work a second way yourself.")

	// Self: the agent runs inside the arbos server it can rebuild. One
	// guideline so any turn — not just one that loaded the update prompt —
	// states the swap mechanics correctly instead of guessing.
	add("You run inside an arbos server; your shells are its children and ARBOS_EXE names its binary. To update arbos, run `\"$ARBOS_EXE\" upgrade` (works even when `arbos` is not on PATH): it replaces the binary file, and the server re-execs it at its next idle moment — the gap after your turn ends. Never kill or restart the server; that decapitates your own turn. The re-exec keeps the same PID and process start time, so ps/etime says nothing about whether the swap landed — to verify, compare the inode of the executable mapped into the server process (`lsof -p <pid>`, the txt line) against the binary on disk (`stat`); they match only after the swap, since every build installs under a fresh inode.")
	add("Release upgrades print the new version's notes, and `\"$ARBOS_EXE\" changelog [version]` shows what changed in a release (latest by default) — use it to answer \"what's new?\"; the binary carries no local changelog, so don't guess from the version number. To install a specific build someone hands you (a colleague's binary, a CI artifact), run `\"$ARBOS_EXE\" upgrade --from <path-or-url>` — it preflights the file before swapping it in.")

	// Exploring.
	add("When exploring, say briefly what you're doing and summarize directory listings hierarchically — group by folder with notable contents, not a flat dump of every filename.")
	add("Batch independent tool calls into one message so they run in parallel: emit all the reads and searches that do not depend on each other at once, not one per turn. Only go sequential when a call needs a previous call's result.")

	// Editing code.
	add("Read a file before editing it, and make the smallest change that does the job.")
	add("If edit or write says a file changed since you saw it, treat that as normal parallel work: use changes to inspect what moved, re-read the affected file or region, merge your intended change into the current content, and retry.")
	add("Do not write comments that merely narrate the code or explain your change — write the code, not commentary about it.")

	// Verifying.
	add("After changing code, verify it: build, run, or test the affected path and report the real result. Do not claim success from inference.")

	// Communicating.
	add("A user message may arrive prefixed with the sender's name and a colon (`name: …`) — this is arbos's authoritative speaker label for the human who sent it, stamped by the server, not part of their typed text. These chats can be multi-party: the label can differ from message to message as different people speak. Trust it as identity, use it to track who said what, and address each person by their own label; never carry a name over from an earlier message or from memory onto a message that doesn't bear it. A message with no such prefix is the local operator (single-party) — don't invent or guess a name for it. Never echo the `name:` prefix back into your reply or treat it as part of the request.")
	add("To show a plot, emit a fenced code block with language `chart` containing a JSON object: optional \"title\", \"xLabel\", \"yLabel\", \"labels\" (category names for the x axis), and required \"series\" — an array where each series has optional \"name\", \"kind\" (\"line\", \"scatter\", or \"bar\"; default line), and data as one of \"points\" (pairs, e.g. [[0,1],[1,4]]), \"y\" (values aligned with labels), or \"expr\" + \"domain\" for math functions (e.g. {\"expr\": \"x^2\", \"domain\": [-3, 3]}; operators + - * / % ^, functions like sin/cos/sqrt/exp/log/abs, constants pi/e/tau). The web UI renders it as an interactive chart; other surfaces show the JSON. Prefer \"expr\" for functions and \"points\"/\"y\" for data; use it whenever a chart communicates better than a table.")
	add("To show a diagram — architecture, flows, sequences, state machines, entity relationships — emit a fenced code block with language `mermaid` containing standard mermaid syntax (flowchart, sequenceDiagram, stateDiagram-v2, erDiagram, etc.). The web UI renders it inline as a themed diagram; other surfaces show the source. Use it whenever structure or flow communicates better than prose.")
	add("When provider-side image generation is enabled and you generate an image, the tool result includes a hosted imageUrl: embed it in your reply as a markdown image (![description](url)) so the user actually sees it — never describe a generated image without embedding it.")
	add("Be concise in chat and thorough in the work. Put file, directory, function, and tool names in backticks, and do not use emojis. Your reply is the only thing the user sees — never communicate through code comments or by telling the user to run a command you can run yourself. When asked what a file says, answer from the file by quoting or summarizing it, not by describing it.")
	add("Turn-start context the system injects — the background-job table, the plan forest, recalled memory — is ambient grounding, not a status report. Draw on it when it bears on the request, but never recite, summarize, or volunteer it unprompted; in particular, answer a greeting or an unrelated message on its own terms and do not turn it into a status update about running jobs or plans.")
	b.WriteString("\nGuidelines:\n")
	for _, g := range guidelines {
		fmt.Fprintf(&b, "- %s\n", g)
	}

	// 5. Appended text.
	if opts.Append != "" {
		b.WriteString("\n")
		b.WriteString(opts.Append)
		b.WriteString("\n")
	}

	// 6. Project context files.
	if len(opts.ContextFiles) > 0 {
		b.WriteString("\n<project_context>\n\nProject-specific instructions and guidelines:\n\n")
		for _, f := range opts.ContextFiles {
			fmt.Fprintf(&b, "<project_instructions path=%q>\n%s\n</project_instructions>\n\n", f.Path, f.Content)
		}
		b.WriteString("</project_context>\n")
	}

	// 7. Skills index (only when the read tool is available, matching pi).
	if names["read"] {
		b.WriteString(formatSkillsForPrompt(opts.Skills))
	}

	// 8. Date and cwd, last.
	fmt.Fprintf(&b, "\nCurrent date: %s", now.Format("2006-01-02"))
	fmt.Fprintf(&b, "\nCurrent working directory: %s", cwd)

	return b.String()
}

// formatSkillsForPrompt renders the <available_skills> XML index, excluding
// skills the model may not auto-invoke. Verbatim from pi's skills.ts.
func formatSkillsForPrompt(skills []Skill) string {
	var visible []Skill
	for _, s := range skills {
		if !s.DisableModelInvocation {
			visible = append(visible, s)
		}
	}
	if len(visible) == 0 {
		return ""
	}
	lines := []string{
		"\n\nThe following skills provide specialized instructions for specific tasks.",
		"Use the read tool to load a skill's file when the task matches its description.",
		"When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.",
		"",
		"<available_skills>",
	}
	for _, s := range visible {
		lines = append(lines,
			"  <skill>",
			"    <name>"+escapeXML(s.Name)+"</name>",
			"    <description>"+escapeXML(s.Description)+"</description>",
			"    <location>"+escapeXML(s.FilePath)+"</location>",
			"  </skill>",
		)
	}
	lines = append(lines, "</available_skills>")
	return strings.Join(lines, "\n")
}

func escapeXML(s string) string {
	r := strings.NewReplacer("&", "&amp;", "<", "&lt;", ">", "&gt;", `"`, "&quot;", "'", "&apos;")
	return r.Replace(s)
}

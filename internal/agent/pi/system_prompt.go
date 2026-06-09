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

// BuildSystemPrompt assembles pi's coding system prompt. It reproduces pi's
// assembly mechanics (system-prompt.ts) step for step: persona, the
// snippet-driven tools list, de-duplicated per-tool guidelines plus two
// always-on bullets, the project-context block, the skills index, and the
// trailing date and cwd. The persona is rewritten for arbos; pi's "you are
// inside pi, read pi docs at <path>" block is intentionally dropped (it
// hardcodes pi-repo paths). See ADR-0024.
func BuildSystemPrompt(opts PromptOptions) string {
	now := opts.Now
	if now.IsZero() {
		now = time.Now()
	}
	cwd := strings.ReplaceAll(opts.Cwd, "\\", "/")

	var b strings.Builder

	// 1. Persona (arbos).
	b.WriteString("You are an expert coding assistant operating inside arbos, a coding agent. ")
	b.WriteString("You help users by reading files, executing commands, editing code, and writing new files.\n\n")

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
	add("Be concise in your responses")
	add("Show file paths clearly when working with files")
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

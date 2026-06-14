package pi

import (
	"embed"
	"io/fs"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"sync"
)

// Prompt templates (slash commands), ported from pi's prompt-templates.ts. A
// `.md` file under a prompts directory becomes a template whose name is the file
// basename; a user message of the form `/name args...` expands to the template
// body with argument substitution before it becomes a turn.

const projectPromptsDir = ".arbos/prompts"

// The default command set ships inside the binary so a fresh deployment has a
// working `/` menu with no project or user prompt files on disk. Built-ins sit
// at the bottom of the precedence order (project > user > built-in): dropping
// a same-named file in either scope overrides the shipped version.
//
//go:embed prompts/*.md
var builtinPromptsFS embed.FS

var builtinTemplates = sync.OnceValue(func() []PromptTemplate {
	entries, err := fs.ReadDir(builtinPromptsFS, "prompts")
	if err != nil {
		return nil
	}
	var out []PromptTemplate
	for _, e := range entries {
		raw, err := fs.ReadFile(builtinPromptsFS, "prompts/"+e.Name())
		if err != nil {
			continue
		}
		// Path stays empty: there is no file to open, and the popup's edit
		// affordance then falls through to its create flow, which writes a
		// project-scope override — exactly how a built-in gets customized.
		// Raw carries the shipped definition so that create flow can open the
		// full template for in-place editing rather than a blank skeleton.
		t := parseTemplate(e.Name(), string(raw), "")
		t.Raw = string(raw)
		out = append(out, t)
	}
	return out
})

// PromptTemplate is a loaded slash-command template.
type PromptTemplate struct {
	Name         string
	Description  string
	ArgumentHint string
	Content      string
	// Raw is the unparsed template file (frontmatter + body). Built-ins carry
	// it so the editor can open the full shipped definition for in-place
	// editing even though there is no file on disk; on-disk templates leave
	// it empty (the editor reads them back from their Path).
	Raw string
	// Path is the source file the template was loaded from (absolute), so a
	// frontend can offer to open it for editing.
	Path string
}

// LoadPromptTemplates loads templates from the project scope (cwd/.arbos/prompts),
// the user scope (agentDir/prompts), and the built-in set shipped in the binary,
// in that precedence order. A name present in more than one scope resolves to
// the highest scope's version — the repo's conventions override personal
// defaults, which override what ships — and appears once in the listing (names
// compare case-insensitively, same as expansion).
func LoadPromptTemplates(cwd, agentDir string) []PromptTemplate {
	out := loadTemplatesFromDir(filepath.Join(cwd, projectPromptsDir))
	seen := make(map[string]bool, len(out))
	for _, t := range out {
		seen[strings.ToLower(t.Name)] = true
	}
	add := func(templates []PromptTemplate) {
		for _, t := range templates {
			if !seen[strings.ToLower(t.Name)] {
				seen[strings.ToLower(t.Name)] = true
				out = append(out, t)
			}
		}
	}
	if agentDir != "" {
		add(loadTemplatesFromDir(filepath.Join(agentDir, "prompts")))
	}
	add(builtinTemplates())
	return out
}

func loadTemplatesFromDir(dir string) []PromptTemplate {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil
	}
	var out []PromptTemplate
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".md") {
			continue
		}
		raw, err := os.ReadFile(filepath.Join(dir, e.Name()))
		if err != nil {
			continue
		}
		out = append(out, parseTemplate(e.Name(), string(raw), filepath.Join(dir, e.Name())))
	}
	return out
}

// parseTemplate turns one `.md` file into a template: name from the filename,
// description from frontmatter (or the body's first line, truncated), body
// with frontmatter stripped.
func parseTemplate(filename, raw, path string) PromptTemplate {
	fm, body := splitFrontmatter(raw)
	desc := fm["description"]
	if desc == "" {
		if first := firstNonEmptyLine(body); first != "" {
			desc = first
			if len(first) > 60 {
				desc = first[:60] + "..."
			}
		}
	}
	return PromptTemplate{
		Name:         strings.TrimSuffix(filename, ".md"),
		Description:  desc,
		ArgumentHint: fm["argument-hint"],
		Content:      body,
		Path:         path,
	}
}

// TemplateExpander returns an expand function for the control seam. Templates
// are reloaded on each slash-prefixed message, so editing a prompt file takes
// effect on the next message without a host restart; non-slash input never
// touches the disk.
func TemplateExpander(cwd, agentDir string) func(string) string {
	return func(s string) string {
		if !strings.HasPrefix(s, "/") {
			return s
		}
		return ExpandPromptTemplate(s, LoadPromptTemplates(cwd, agentDir))
	}
}

var slashRe = regexp.MustCompile(`^/([^\s]+)(?:\s+([\s\S]*))?$`)

// ExpandPromptTemplate expands a leading `/name args` into the matching
// template's body with argument substitution. Non-template input is returned
// unchanged.
func ExpandPromptTemplate(text string, templates []PromptTemplate) string {
	if !strings.HasPrefix(text, "/") {
		return text
	}
	m := slashRe.FindStringSubmatch(text)
	if m == nil {
		return text
	}
	name, argsStr := m[1], m[2]
	for _, t := range templates {
		// Case-insensitive: the composer's popup filters case-insensitively,
		// so "/Plan" must expand the same template it offered.
		if strings.EqualFold(t.Name, name) {
			return substituteArgs(t.Content, argsStr, parseCommandArgs(argsStr))
		}
	}
	return text
}

// parseCommandArgs splits an argument string with shell-style single and double
// quotes.
func parseCommandArgs(s string) []string {
	var args []string
	var cur strings.Builder
	var quote rune
	for _, c := range s {
		switch {
		case quote != 0:
			if c == quote {
				quote = 0
			} else {
				cur.WriteRune(c)
			}
		case c == '"' || c == '\'':
			quote = c
		case c == ' ' || c == '\t':
			if cur.Len() > 0 {
				args = append(args, cur.String())
				cur.Reset()
			}
		default:
			cur.WriteRune(c)
		}
	}
	if cur.Len() > 0 {
		args = append(args, cur.String())
	}
	return args
}

var (
	posRe   = regexp.MustCompile(`\$(\d+)`)
	sliceRe = regexp.MustCompile(`\$\{@:(\d+)(?::(\d+))?\}`)
)

// substituteArgs replaces $1/$2, ${@:N}/${@:N:L}, $ARGUMENTS, and $@ in order,
// matching pi. Positional first so wildcard replacements are not re-substituted.
// $ARGUMENTS and $@ substitute the raw argument string, not the re-joined
// parsed tokens: prose like "don't break the user's API" must reach the
// template verbatim — quote parsing exists for positional args only.
func substituteArgs(content, raw string, args []string) string {
	at := func(i int) string {
		if i >= 0 && i < len(args) {
			return args[i]
		}
		return ""
	}
	out := posRe.ReplaceAllStringFunc(content, func(m string) string {
		n, _ := strconv.Atoi(m[1:])
		return at(n - 1)
	})
	out = sliceRe.ReplaceAllStringFunc(out, func(m string) string {
		g := sliceRe.FindStringSubmatch(m)
		start, _ := strconv.Atoi(g[1])
		start--
		if start < 0 {
			start = 0
		}
		if start > len(args) {
			start = len(args)
		}
		if g[2] != "" {
			length, _ := strconv.Atoi(g[2])
			end := start + length
			if end > len(args) {
				end = len(args)
			}
			return strings.Join(args[start:end], " ")
		}
		return strings.Join(args[start:], " ")
	})
	all := strings.TrimSpace(raw)
	out = strings.ReplaceAll(out, "$ARGUMENTS", all)
	out = strings.ReplaceAll(out, "$@", all)
	return out
}

func splitFrontmatter(content string) (map[string]string, string) {
	content = strings.ReplaceAll(strings.ReplaceAll(content, "\r\n", "\n"), "\r", "\n")
	if !strings.HasPrefix(content, "---") {
		return map[string]string{}, strings.TrimSpace(content)
	}
	end := strings.Index(content[3:], "\n---")
	if end == -1 {
		return map[string]string{}, strings.TrimSpace(content)
	}
	fm := parseFrontmatter(content)
	body := strings.TrimSpace(content[3+end+4:])
	return fm, body
}

func firstNonEmptyLine(s string) string {
	for _, line := range strings.Split(s, "\n") {
		if t := strings.TrimSpace(line); t != "" {
			return t
		}
	}
	return ""
}

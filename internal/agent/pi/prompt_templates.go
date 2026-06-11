package pi

import (
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
)

// Prompt templates (slash commands), ported from pi's prompt-templates.ts. A
// `.md` file under a prompts directory becomes a template whose name is the file
// basename; a user message of the form `/name args...` expands to the template
// body with argument substitution before it becomes a turn.

const projectPromptsDir = ".arbos/prompts"

// PromptTemplate is a loaded slash-command template.
type PromptTemplate struct {
	Name         string
	Description  string
	ArgumentHint string
	Content      string
	// Path is the source file the template was loaded from (absolute), so a
	// frontend can offer to open it for editing.
	Path string
}

// LoadPromptTemplates loads templates from the project scope (cwd/.arbos/prompts)
// and the user scope (agentDir/prompts). A name present in both scopes resolves
// to the project's version — the repo's conventions override personal defaults,
// matching how context files and skills already layer — and appears once in the
// listing (names compare case-insensitively, same as expansion).
func LoadPromptTemplates(cwd, agentDir string) []PromptTemplate {
	out := loadTemplatesFromDir(filepath.Join(cwd, projectPromptsDir))
	if agentDir == "" {
		return out
	}
	seen := make(map[string]bool, len(out))
	for _, t := range out {
		seen[strings.ToLower(t.Name)] = true
	}
	for _, t := range loadTemplatesFromDir(filepath.Join(agentDir, "prompts")) {
		if !seen[strings.ToLower(t.Name)] {
			out = append(out, t)
		}
	}
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
		fm, body := splitFrontmatter(string(raw))
		desc := fm["description"]
		if desc == "" {
			if first := firstNonEmptyLine(body); first != "" {
				desc = first
				if len(first) > 60 {
					desc = first[:60] + "..."
				}
			}
		}
		out = append(out, PromptTemplate{
			Name:         strings.TrimSuffix(e.Name(), ".md"),
			Description:  desc,
			ArgumentHint: fm["argument-hint"],
			Content:      body,
			Path:         filepath.Join(dir, e.Name()),
		})
	}
	return out
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

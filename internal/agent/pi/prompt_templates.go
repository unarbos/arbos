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
}

// LoadPromptTemplates loads templates from the user scope (agentDir/prompts) and
// the project scope (cwd/.arbos/prompts).
func LoadPromptTemplates(cwd, agentDir string) []PromptTemplate {
	var out []PromptTemplate
	if agentDir != "" {
		out = append(out, loadTemplatesFromDir(filepath.Join(agentDir, "prompts"))...)
	}
	out = append(out, loadTemplatesFromDir(filepath.Join(cwd, projectPromptsDir))...)
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
		})
	}
	return out
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
		if t.Name == name {
			return substituteArgs(t.Content, parseCommandArgs(argsStr))
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
func substituteArgs(content string, args []string) string {
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
	all := strings.Join(args, " ")
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

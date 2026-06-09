package pi

import (
	"os"
	"path/filepath"
	"strings"
)

// Skill discovery, ported from pi's skills.ts. SKILL.md files are discovered
// across a user scope (agentDir/skills) and a project scope (cwd/.arbos/skills).
// A directory containing SKILL.md is a skill root and is not recursed into;
// otherwise direct .md children of a scope root are loaded and subdirectories
// are searched for SKILL.md. On a name collision the first-loaded wins, and the
// user scope is loaded first (pi's behavior).
//
// Deviations, documented: frontmatter is parsed for the simple scalar fields the
// skill spec uses (no general YAML dependency), and gitignore filtering inside
// skills directories is not applied (node_modules and dotfile dirs are skipped).

const projectSkillsDir = ".arbos/skills"

// LoadSkills discovers skills from the user scope then the project scope,
// de-duplicated by name (first wins).
func LoadSkills(cwd, agentDir string) []Skill {
	var out []Skill
	seen := map[string]bool{}
	add := func(skills []Skill) {
		for _, s := range skills {
			if seen[s.Name] {
				continue
			}
			seen[s.Name] = true
			out = append(out, s)
		}
	}
	if agentDir != "" {
		add(loadSkillsFromDir(filepath.Join(agentDir, "skills"), true))
	}
	add(loadSkillsFromDir(filepath.Join(cwd, projectSkillsDir), true))
	return out
}

func loadSkillsFromDir(dir string, includeRootMd bool) []Skill {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil
	}
	// A directory that is itself a skill root: load its SKILL.md and stop.
	for _, e := range entries {
		if e.Name() == "SKILL.md" && !e.IsDir() {
			if s, ok := loadSkillFile(filepath.Join(dir, "SKILL.md")); ok {
				return []Skill{s}
			}
			return nil
		}
	}
	var out []Skill
	for _, e := range entries {
		name := e.Name()
		if strings.HasPrefix(name, ".") || name == "node_modules" {
			continue
		}
		full := filepath.Join(dir, name)
		if e.IsDir() {
			out = append(out, loadSkillsFromDir(full, false)...)
			continue
		}
		if includeRootMd && strings.HasSuffix(name, ".md") {
			if s, ok := loadSkillFile(full); ok {
				out = append(out, s)
			}
		}
	}
	return out
}

func loadSkillFile(path string) (Skill, bool) {
	raw, err := os.ReadFile(path)
	if err != nil {
		return Skill{}, false
	}
	fm := parseFrontmatter(string(raw))
	desc := strings.TrimSpace(fm["description"])
	if desc == "" {
		return Skill{}, false // description is required (pi drops these)
	}
	name := fm["name"]
	if name == "" {
		name = filepath.Base(filepath.Dir(path)) // fall back to parent dir name
	}
	return Skill{
		Name:                   name,
		Description:            desc,
		FilePath:               path,
		DisableModelInvocation: strings.EqualFold(fm["disable-model-invocation"], "true"),
	}, true
}

// parseFrontmatter reads a leading `---`-fenced YAML block and returns its
// simple `key: value` scalar pairs (quotes stripped). It is intentionally
// minimal: skill frontmatter uses only scalar name/description/flag fields.
func parseFrontmatter(content string) map[string]string {
	content = strings.ReplaceAll(strings.ReplaceAll(content, "\r\n", "\n"), "\r", "\n")
	if !strings.HasPrefix(content, "---") {
		return map[string]string{}
	}
	end := strings.Index(content[3:], "\n---")
	if end == -1 {
		return map[string]string{}
	}
	block := content[4 : 3+end]
	out := map[string]string{}
	for _, line := range strings.Split(block, "\n") {
		i := strings.Index(line, ":")
		if i <= 0 {
			continue
		}
		key := strings.TrimSpace(line[:i])
		val := strings.TrimSpace(line[i+1:])
		val = strings.Trim(val, `"'`)
		out[key] = val
	}
	return out
}

package codingspec

import (
	"os"
	"path/filepath"
	"strings"
)

// ignoreRule is one parsed .gitignore line.
type ignoreRule struct {
	pattern  string
	negate   bool
	dirOnly  bool // trailing '/': only matches directories
	anchored bool // non-trailing '/': matched relative to the .gitignore's dir
}

// matches reports whether the rule applies to rel, a slash-separated path
// relative to the directory holding the rule's .gitignore.
func (r ignoreRule) matches(rel string, isDir bool) bool {
	if r.dirOnly && !isDir {
		return false
	}
	p := r.pattern
	if !r.anchored {
		p = "**/" + p
	}
	return matchGlob(p, rel)
}

// parseIgnoreRules parses .gitignore content. Unsupported edge: backslash-
// escaped trailing spaces (escaped leading '#' and '!' are handled).
func parseIgnoreRules(data []byte) []ignoreRule {
	var rules []ignoreRule
	for _, raw := range strings.Split(string(data), "\n") {
		line := strings.TrimRight(strings.TrimSuffix(raw, "\r"), " ")
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		var r ignoreRule
		if strings.HasPrefix(line, "!") {
			r.negate = true
			line = line[1:]
		}
		line = strings.TrimPrefix(line, `\`)
		if strings.HasSuffix(line, "/") {
			r.dirOnly = true
			line = strings.TrimSuffix(line, "/")
		}
		if strings.Contains(line, "/") {
			r.anchored = true
			line = strings.TrimPrefix(line, "/")
		}
		if line == "" {
			continue
		}
		r.pattern = line
		rules = append(rules, r)
	}
	return rules
}

type ignoreScope struct {
	dir   string // absolute directory containing the .gitignore
	rules []ignoreRule
}

// ignoreMatcher is a stack of .gitignore scopes maintained during a DFS,
// outermost first. Pruning ignored directories at walk time gives git's
// "cannot re-include inside an excluded directory" semantics for free.
type ignoreMatcher struct {
	scopes []ignoreScope
}

// push loads dir/.gitignore if present and returns whether a scope was pushed
// (callers pop only when it was).
func (m *ignoreMatcher) push(dir string) bool {
	data, err := os.ReadFile(filepath.Join(dir, ".gitignore"))
	if err != nil {
		return false
	}
	rules := parseIgnoreRules(data)
	if len(rules) == 0 {
		return false
	}
	m.scopes = append(m.scopes, ignoreScope{dir: dir, rules: rules})
	return true
}

func (m *ignoreMatcher) pop() {
	m.scopes = m.scopes[:len(m.scopes)-1]
}

// ignored reports whether abs is excluded. Scopes are evaluated outermost
// first and rules in file order, with the last match winning, per gitignore
// precedence.
func (m *ignoreMatcher) ignored(abs string, isDir bool) bool {
	excluded := false
	for _, s := range m.scopes {
		rel, err := filepath.Rel(s.dir, abs)
		if err != nil || rel == "." || strings.HasPrefix(rel, "..") {
			continue
		}
		rel = filepath.ToSlash(rel)
		for _, r := range s.rules {
			if r.matches(rel, isDir) {
				excluded = !r.negate
			}
		}
	}
	return excluded
}

// loadAncestorIgnores pushes .gitignore scopes from the enclosing git repo
// root down to searchPath's parent, so searching a subtree still honors rules
// defined above it. Outside a git repo no ancestor scopes apply.
func loadAncestorIgnores(m *ignoreMatcher, searchPath string) {
	var chain []string
	dir := searchPath
	for {
		parent := filepath.Dir(dir)
		if parent == dir {
			return // hit the filesystem root without finding a repo
		}
		dir = parent
		chain = append(chain, dir)
		if _, err := os.Stat(filepath.Join(dir, ".git")); err == nil {
			break
		}
	}
	for i := len(chain) - 1; i >= 0; i-- {
		m.push(chain[i])
	}
}

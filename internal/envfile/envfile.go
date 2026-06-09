// Package envfile loads simple KEY=VALUE .env files for local dev.
// Existing environment variables are never overwritten.
package envfile

import (
	"bufio"
	"os"
	"path/filepath"
	"strings"
)

// Load reads paths in order; later files do not override earlier-set vars
// (or vars already in the process environment).
func Load(paths ...string) {
	seen := make(map[string]bool)
	for _, entry := range os.Environ() {
		if i := strings.IndexByte(entry, '='); i > 0 {
			seen[entry[:i]] = true
		}
	}
	for _, path := range paths {
		loadOne(path, seen)
	}
}

// LoadDefault loads ~/.config/arbos/.env then ./.env from cwd.
func LoadDefault(agentDir string) {
	cwd, _ := os.Getwd()
	var paths []string
	if agentDir != "" {
		paths = append(paths, filepath.Join(agentDir, ".env"))
	}
	paths = append(paths, filepath.Join(cwd, ".env"))
	Load(paths...)
}

func loadOne(path string, seen map[string]bool) {
	f, err := os.Open(path)
	if err != nil {
		return
	}
	defer func() { _ = f.Close() }()

	sc := bufio.NewScanner(f)
	for sc.Scan() {
		line := strings.TrimSpace(sc.Text())
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		key, val, ok := strings.Cut(line, "=")
		if !ok {
			continue
		}
		key = strings.TrimSpace(key)
		val = strings.TrimSpace(val)
		val = strings.Trim(val, `"'`)
		if key == "" || seen[key] {
			continue
		}
		_ = os.Setenv(key, val)
		seen[key] = true
	}
}

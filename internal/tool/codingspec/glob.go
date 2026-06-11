package codingspec

import (
	"path"
	"strings"
)

// matchGlob reports whether the slash-separated path p matches pattern.
// Within a segment, '*', '?', and '[...]' have path.Match semantics and never
// cross '/'. A segment that is exactly "**" matches zero or more whole
// segments. This is the one glob engine behind both find's pattern matching
// and gitignore rule matching.
func matchGlob(pattern, p string) bool {
	return matchSegs(strings.Split(pattern, "/"), strings.Split(p, "/"))
}

func matchSegs(pat, segs []string) bool {
	for len(pat) > 0 {
		if pat[0] == "**" {
			for len(pat) > 0 && pat[0] == "**" {
				pat = pat[1:]
			}
			if len(pat) == 0 {
				return true
			}
			for i := 0; i <= len(segs); i++ {
				if matchSegs(pat, segs[i:]) {
					return true
				}
			}
			return false
		}
		if len(segs) == 0 {
			return false
		}
		if ok, err := path.Match(pat[0], segs[0]); err != nil || !ok {
			return false
		}
		pat, segs = pat[1:], segs[1:]
	}
	return len(segs) == 0
}

// validGlob reports whether every segment of pattern is a well-formed
// path.Match pattern, so malformed globs fail loudly instead of silently
// matching nothing.
func validGlob(pattern string) bool {
	for _, seg := range strings.Split(pattern, "/") {
		if _, err := path.Match(seg, "x"); err != nil {
			return false
		}
	}
	return true
}

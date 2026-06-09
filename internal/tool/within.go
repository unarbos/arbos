package tool

import "path/filepath"

// WithinRoot reports whether path is root or a descendant of root. Both paths
// should be absolute; callers typically Abs() first.
func WithinRoot(path, root string) bool {
	path = filepath.Clean(path)
	root = filepath.Clean(root)
	return withinRoot(path, root)
}

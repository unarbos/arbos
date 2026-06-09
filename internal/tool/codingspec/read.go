package codingspec

import (
	"context"
	"encoding/base64"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/tool"
)

// ReadArgs are the arguments to read.
type ReadArgs struct {
	Path   string `json:"path" desc:"Path to the file to read, relative to the workspace root."`
	Offset int    `json:"offset,omitempty" desc:"Line number to start reading from (1-indexed)."`
	Limit  int    `json:"limit,omitempty" desc:"Maximum number of lines to read."`
}

// readSpec reads a file, returning text with pi's actionable truncation and
// continuation notices, or an image as a content block. Faithful to pi's read
// tool. Deviations, deliberate and documented in the plan: image type is
// detected by extension (pi uses magic bytes), images are not auto-resized (no
// image lib in the kernel), and the non-vision downgrade note is omitted because
// the tool layer does not know the active model's vision capability.
func readSpec(root string, ledger *readLedger) tool.Spec {
	spec := tool.NewRichSpec("read",
		fmt.Sprintf("Read the contents of a file. Supports text files and images (jpg, png, gif, webp). Images are sent as attachments. Text lines are prefixed with their 1-based line number as `<n>|` — that prefix is display metadata, not part of the file. For text files, output is truncated to %d lines or %dKB (whichever is hit first). Use offset/limit for large files. When you need the full file, continue with offset until complete.", DefaultMaxLines, DefaultMaxBytes/1024),
		true,
		func(ctx context.Context, a ReadArgs) (tool.Result, error) {
			abs, err := tool.Resolve(root, a.Path)
			if err != nil {
				return tool.Result{}, err
			}
			data, err := os.ReadFile(abs)
			if err != nil {
				return tool.Result{}, fmt.Errorf("read: %w", err)
			}

			if mime := imageMime(a.Path); mime != "" {
				if len(data) > maxImageBytes {
					return tool.Result{}, fmt.Errorf("image file too large (%s); maximum is %s", FormatSize(len(data)), FormatSize(maxImageBytes))
				}
				return tool.Result{
					Content: fmt.Sprintf("Read image file [%s]", mime),
					Blocks: []core.ContentBlock{{
						Type:  core.BlockImage,
						Image: &core.ImageData{Data: base64.StdEncoding.EncodeToString(data), MimeType: mime},
					}},
				}, nil
			}

			allLines := strings.Split(string(data), "\n")
			totalFileLines := len(allLines)
			startLine := 0
			if a.Offset > 0 {
				startLine = a.Offset - 1
			}
			startLineDisplay := startLine + 1
			if startLine >= len(allLines) {
				return tool.Result{}, fmt.Errorf("Offset %d is beyond end of file (%d lines total)", a.Offset, len(allLines))
			}

			var selected string
			userLimited := 0
			if a.Limit > 0 {
				end := startLine + a.Limit
				if end > len(allLines) {
					end = len(allLines)
				}
				selected = strings.Join(allLines[startLine:end], "\n")
				userLimited = end - startLine
			} else {
				selected = strings.Join(allLines[startLine:], "\n")
			}

			tr := TruncateHead(selected, DefaultMaxLines, DefaultMaxBytes)
			// Number the shown lines after truncation so the byte/line budget stays
			// a measure of real file content (and the continuation math below is
			// unaffected). The first shown line is file line startLineDisplay.
			numbered := numberLines(tr.Content, startLineDisplay)

			// Context ledger: if these exact lines were already shown this turn and
			// the file is unchanged, skip re-emitting them — return a pointer
			// instead of re-spending the window. A version change shows fresh
			// content with a note. tr.OutputLines == 0 (the first-line-too-large
			// case below) shows nothing, so it never touches the ledger.
			var changeNote string
			if tr.OutputLines > 0 {
				shownStart := startLineDisplay
				shownEnd := shownStart + tr.OutputLines - 1
				switch v := ledger.reconcile(turnFromContext(ctx), abs, fileVersion(data), shownStart, shownEnd); {
				case v.redundant:
					return tool.Result{Content: fmt.Sprintf(
						"[Lines %d-%d of %s were already shown this turn and are unchanged — re-read skipped to save context. The earlier output still applies; use a different offset to see more.]",
						shownStart, shownEnd, a.Path)}, nil
				case v.fileChanged:
					changeNote = fmt.Sprintf("[%s changed since it was last read this turn; showing current content.]\n\n", a.Path)
				}
			}

			var out string
			switch {
			case tr.FirstLineExceedsLimit:
				firstLineSize := FormatSize(len(allLines[startLine]))
				out = fmt.Sprintf("[Line %d is %s, exceeds %s limit. Use bash: sed -n '%dp' %s | head -c %d]",
					startLineDisplay, firstLineSize, FormatSize(DefaultMaxBytes), startLineDisplay, a.Path, DefaultMaxBytes)
			case tr.Truncated:
				endLineDisplay := startLineDisplay + tr.OutputLines - 1
				nextOffset := endLineDisplay + 1
				if tr.TruncatedBy == "lines" {
					out = numbered + fmt.Sprintf("\n\n[Showing lines %d-%d of %d. Use offset=%d to continue.]",
						startLineDisplay, endLineDisplay, totalFileLines, nextOffset)
				} else {
					out = numbered + fmt.Sprintf("\n\n[Showing lines %d-%d of %d (%s limit). Use offset=%d to continue.]",
						startLineDisplay, endLineDisplay, totalFileLines, FormatSize(DefaultMaxBytes), nextOffset)
				}
			case userLimited > 0 && startLine+userLimited < len(allLines):
				remaining := len(allLines) - (startLine + userLimited)
				nextOffset := startLine + userLimited + 1
				out = numbered + fmt.Sprintf("\n\n[%d more lines in file. Use offset=%d to continue.]", remaining, nextOffset)
			default:
				out = numbered
			}
			return tool.Result{Content: changeNote + out}, nil
		})
	// A read touches one file: declare it so a same-file edit in the same batch
	// is ordered after this read instead of racing it.
	return tool.WithAccess(spec, func(a ReadArgs) core.AccessSet {
		return core.AccessSet{Reads: fileKeys(root, a.Path)}
	})
}

// fileKeys returns the canonical resource key(s) for a workspace-relative path:
// the absolute, symlink-resolved path Resolve produces, so two spellings of the
// same file conflict. A path that cannot be resolved yields an unbounded marker
// via an empty slice's caller (read returns it as a read with no key, which is
// harmless; write/edit treat an unresolved path as unbounded — see their specs).
func fileKeys(root, path string) []string {
	abs, err := tool.Resolve(root, path)
	if err != nil {
		return nil
	}
	return []string{abs}
}

// lineNumberWidth is the minimum gutter width for read's line numbers. Numbers
// wider than this (files past 10^6-1 lines) are not truncated — they push the
// separator right rather than misreport a line. A fixed minimum keeps the gutter
// aligned without pre-scanning the block to find the largest number.
const lineNumberWidth = 6

// numberLines prefixes each line of content with its 1-based file line number as
// "<n>|<line>" — the addressable coordinate system read shares with edit and
// with offset/continue. startLine is the file line number of content's first
// line. It walks the string once, emitting prefixes inline, so it allocates a
// single buffer rather than an intermediate line slice. Empty content yields
// empty output, leaving an empty or fully-truncated read unchanged. A trailing
// newline is the previous line's terminator, not a new empty line, so it is not
// numbered (matching how editors gutter a newline-terminated file).
func numberLines(content string, startLine int) string {
	if content == "" {
		return ""
	}
	var b strings.Builder
	b.Grow(len(content) + (lineNumberWidth+1)*(strings.Count(content, "\n")+1))
	n := startLine
	start := 0
	for i := 0; i < len(content); i++ {
		if content[i] == '\n' {
			writeNumberedLine(&b, n, content[start:i])
			b.WriteByte('\n')
			n++
			start = i + 1
		}
	}
	if start < len(content) {
		writeNumberedLine(&b, n, content[start:])
	}
	return b.String()
}

// writeNumberedLine emits "<right-aligned n>|<line>" without fmt's reflection
// overhead, padding to lineNumberWidth with spaces.
func writeNumberedLine(b *strings.Builder, n int, line string) {
	num := strconv.Itoa(n)
	for pad := lineNumberWidth - len(num); pad > 0; pad-- {
		b.WriteByte(' ')
	}
	b.WriteString(num)
	b.WriteByte('|')
	b.WriteString(line)
}

func imageMime(path string) string {
	switch strings.ToLower(filepath.Ext(path)) {
	case ".png":
		return "image/png"
	case ".jpg", ".jpeg":
		return "image/jpeg"
	case ".gif":
		return "image/gif"
	case ".webp":
		return "image/webp"
	}
	return ""
}

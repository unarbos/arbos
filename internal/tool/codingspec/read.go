package codingspec

import (
	"context"
	"encoding/base64"
	"fmt"
	"os"
	"path/filepath"
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
func readSpec(root string) tool.Spec {
	return tool.NewRichSpec("read",
		fmt.Sprintf("Read the contents of a file. Supports text files and images (jpg, png, gif, webp). Images are sent as attachments. For text files, output is truncated to %d lines or %dKB (whichever is hit first). Use offset/limit for large files. When you need the full file, continue with offset until complete.", DefaultMaxLines, DefaultMaxBytes/1024),
		true,
		func(_ context.Context, a ReadArgs) (tool.Result, error) {
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
					out = tr.Content + fmt.Sprintf("\n\n[Showing lines %d-%d of %d. Use offset=%d to continue.]",
						startLineDisplay, endLineDisplay, totalFileLines, nextOffset)
				} else {
					out = tr.Content + fmt.Sprintf("\n\n[Showing lines %d-%d of %d (%s limit). Use offset=%d to continue.]",
						startLineDisplay, endLineDisplay, totalFileLines, FormatSize(DefaultMaxBytes), nextOffset)
				}
			case userLimited > 0 && startLine+userLimited < len(allLines):
				remaining := len(allLines) - (startLine + userLimited)
				nextOffset := startLine + userLimited + 1
				out = tr.Content + fmt.Sprintf("\n\n[%d more lines in file. Use offset=%d to continue.]", remaining, nextOffset)
			default:
				out = tr.Content
			}
			return tool.Result{Content: out}, nil
		})
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

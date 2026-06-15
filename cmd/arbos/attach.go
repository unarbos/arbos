package main

// Image attachment helpers for the TUI: turning a typed/dropped file path or an
// image on the system clipboard into a core.ContentBlock that rides the next
// prompt as PromptIntent.Parts. The backend (providers with Vision:true) already
// serializes these; this file is the terminal frontend's only entry point for them.

import (
	"context"
	"encoding/base64"
	"encoding/hex"
	"errors"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"

	"github.com/unarbos/arbos/internal/core"
)

// supportedImageMIME is the closed set providers accept (mirrors screenshotMime
// in internal/tool/codingspec). Anything else is rejected so we never label
// bytes a provider will 400 on.
func supportedImageMIME(ct string) bool {
	switch ct {
	case "image/png", "image/jpeg", "image/gif", "image/webp":
		return true
	default:
		return false
	}
}

// extractImageAttachments lifts image-file paths out of a typed line, returning
// the remaining prose and the decoded image blocks. A line with no resolvable
// image path comes back unchanged with a nil slice, so ordinary prompts are
// untouched. The existence-on-disk + content-sniff gate in looksLikeImagePath is
// what keeps a word like "screenshot.png" in prose from being mistaken for an
// attachment unless it is a real image file.
func extractImageAttachments(line string) (string, []core.ContentBlock) {
	tokens := splitPathTokens(line)
	var blocks []core.ContentBlock
	var rest []string
	for _, tok := range tokens {
		if mime, ok := looksLikeImagePath(tok); ok {
			if b, err := readImageBlock(tok, mime); err == nil {
				blocks = append(blocks, b)
				continue
			}
		}
		rest = append(rest, tok)
	}
	if len(blocks) == 0 {
		return line, nil
	}
	return strings.TrimSpace(strings.Join(rest, " ")), blocks
}

// splitPathTokens splits on unquoted whitespace while honoring the two ways a
// terminal delivers a path with spaces: backslash escapes (macOS drag-drop turns
// "my shot.png" into "my\ shot.png") and surrounding quotes (some terminals
// quote instead). Escapes and quotes are resolved into the returned token.
func splitPathTokens(line string) []string {
	var tokens []string
	var b strings.Builder
	inTok := false
	var quote rune
	flush := func() {
		if inTok {
			if s := b.String(); s != "" {
				tokens = append(tokens, s)
			}
			b.Reset()
			inTok = false
		}
	}
	rs := []rune(line)
	for i := 0; i < len(rs); i++ {
		c := rs[i]
		switch {
		case c == '\\' && i+1 < len(rs):
			i++
			b.WriteRune(rs[i])
			inTok = true
		case quote != 0:
			if c == quote {
				quote = 0
			} else {
				b.WriteRune(c)
			}
			inTok = true
		case c == '\'' || c == '"':
			quote = c
			inTok = true
		case c == ' ' || c == '\t' || c == '\n' || c == '\r':
			flush()
		default:
			b.WriteRune(c)
			inTok = true
		}
	}
	flush()
	return tokens
}

// looksLikeImagePath reports whether tok resolves to an existing regular file
// whose first bytes sniff as a provider-supported image, returning the canonical
// MIME. Requiring the file to exist is the key discriminator from prose.
func looksLikeImagePath(tok string) (string, bool) {
	p := expandTilde(tok)
	info, err := os.Stat(p)
	if err != nil || !info.Mode().IsRegular() {
		return "", false
	}
	f, err := os.Open(p)
	if err != nil {
		return "", false
	}
	defer func() { _ = f.Close() }()
	head := make([]byte, 512)
	n, _ := f.Read(head)
	mime := http.DetectContentType(head[:n])
	if !supportedImageMIME(mime) {
		return "", false
	}
	return mime, true
}

// readImageBlock reads the file and base64-encodes it into an image ContentBlock.
func readImageBlock(path, mime string) (core.ContentBlock, error) {
	data, err := os.ReadFile(expandTilde(path))
	if err != nil {
		return core.ContentBlock{}, err
	}
	return imageBlock(data, mime), nil
}

func imageBlock(data []byte, mime string) core.ContentBlock {
	return core.ContentBlock{
		Type:  core.BlockImage,
		Image: &core.ImageData{Data: base64.StdEncoding.EncodeToString(data), MimeType: mime},
	}
}

func expandTilde(p string) string {
	if p == "~" {
		if h, err := os.UserHomeDir(); err == nil {
			return h
		}
		return p
	}
	if strings.HasPrefix(p, "~/") {
		if h, err := os.UserHomeDir(); err == nil {
			return filepath.Join(h, p[2:])
		}
	}
	return p
}

var (
	errNoClipboardTool  = errors.New("no clipboard image tool")
	errNoClipboardImage = errors.New("no image in clipboard")
)

// clipboardImage reads an image off the OS clipboard via the platform helper,
// returning the bytes and sniffed MIME. It returns errNoClipboardTool when no
// helper is installed and errNoClipboardImage when the clipboard holds no image,
// so the caller can show the right hint. ctx bounds the shell-out so a hung
// helper cannot freeze the input loop.
func clipboardImage(ctx context.Context) ([]byte, string, error) {
	var (
		data []byte
		err  error
	)
	switch runtime.GOOS {
	case "darwin":
		data, err = clipboardDarwin(ctx)
	case "linux":
		data, err = clipboardLinux(ctx)
	default:
		return nil, "", errNoClipboardTool
	}
	if err != nil {
		return nil, "", err
	}
	if len(data) == 0 {
		return nil, "", errNoClipboardImage
	}
	mime := http.DetectContentType(data)
	if !supportedImageMIME(mime) {
		return nil, "", errNoClipboardImage
	}
	return data, mime, nil
}

func clipboardDarwin(ctx context.Context) ([]byte, error) {
	if path, err := exec.LookPath("pngpaste"); err == nil {
		out, err := exec.CommandContext(ctx, path, "-").Output()
		if err != nil || len(out) == 0 {
			return nil, errNoClipboardImage
		}
		return out, nil
	}
	// Fallback with no extra dependency: ask AppleScript for the clipboard as
	// PNG. It returns «data PNGf<hex>», which we decode.
	out, err := exec.CommandContext(ctx, "osascript", "-e", "the clipboard as «class PNGf»").Output()
	if err != nil {
		return nil, errNoClipboardImage
	}
	return parseOsascriptHex(out)
}

// parseOsascriptHex decodes the «data PNGf...» hex blob osascript prints for a
// PNG clipboard. The 4-char type code ("PNGf") precedes the hex and is stripped
// before hex-decoding.
func parseOsascriptHex(out []byte) ([]byte, error) {
	s := string(out)
	i := strings.Index(s, "«data ")
	if i < 0 {
		return nil, errNoClipboardImage
	}
	s = s[i+len("«data "):]
	j := strings.Index(s, "»")
	if j < 0 {
		return nil, errNoClipboardImage
	}
	body := s[:j]
	if len(body) <= 4 {
		return nil, errNoClipboardImage
	}
	body = body[4:] // drop the "PNGf" type code
	body = strings.Map(func(r rune) rune {
		switch {
		case r >= '0' && r <= '9', r >= 'a' && r <= 'f', r >= 'A' && r <= 'F':
			return r
		default:
			return -1
		}
	}, body)
	data, err := hex.DecodeString(body)
	if err != nil || len(data) == 0 {
		return nil, errNoClipboardImage
	}
	return data, nil
}

func clipboardLinux(ctx context.Context) ([]byte, error) {
	if os.Getenv("WAYLAND_DISPLAY") != "" {
		if path, err := exec.LookPath("wl-paste"); err == nil {
			if out, err := exec.CommandContext(ctx, path, "--type", "image/png").Output(); err == nil && len(out) > 0 {
				return out, nil
			}
		}
	}
	if path, err := exec.LookPath("xclip"); err == nil {
		out, err := exec.CommandContext(ctx, path, "-selection", "clipboard", "-t", "image/png", "-o").Output()
		if err != nil || len(out) == 0 {
			return nil, errNoClipboardImage
		}
		return out, nil
	}
	return nil, errNoClipboardTool
}

// clipboardHint maps a clipboardImage error to a one-line, actionable message.
func clipboardHint(err error) string {
	if errors.Is(err, errNoClipboardImage) {
		return "no image in clipboard"
	}
	switch runtime.GOOS {
	case "darwin":
		return "clipboard image needs pngpaste — brew install pngpaste"
	case "linux":
		return "clipboard image needs xclip or wl-paste"
	default:
		return "clipboard image paste not supported on this platform"
	}
}

package main

import (
	"os"
	"path/filepath"
	"reflect"
	"testing"

	"github.com/unarbos/arbos/internal/core"
)

// pngMagic is the 8-byte PNG signature http.DetectContentType keys on.
var pngMagic = []byte{0x89, 'P', 'N', 'G', '\r', '\n', 0x1a, '\n'}

func TestSplitPathTokens(t *testing.T) {
	cases := []struct {
		in   string
		want []string
	}{
		{"a b c", []string{"a", "b", "c"}},
		{`/tmp/my\ shot.png here`, []string{"/tmp/my shot.png", "here"}},
		{`'/tmp/my shot.png' here`, []string{"/tmp/my shot.png", "here"}},
		{`"/a b/c.png"`, []string{"/a b/c.png"}},
		{"   spaced   out  ", []string{"spaced", "out"}},
		{"''", nil},
	}
	for _, c := range cases {
		if got := splitPathTokens(c.in); !reflect.DeepEqual(got, c.want) {
			t.Errorf("splitPathTokens(%q) = %#v, want %#v", c.in, got, c.want)
		}
	}
}

func TestLooksLikeImagePath(t *testing.T) {
	dir := t.TempDir()
	img := filepath.Join(dir, "shot.png")
	if err := os.WriteFile(img, append(pngMagic, make([]byte, 32)...), 0o600); err != nil {
		t.Fatal(err)
	}
	txt := filepath.Join(dir, "notes.txt")
	if err := os.WriteFile(txt, []byte("just some prose, not an image"), 0o600); err != nil {
		t.Fatal(err)
	}

	if mime, ok := looksLikeImagePath(img); !ok || mime != "image/png" {
		t.Errorf("image file: got (%q, %v), want (image/png, true)", mime, ok)
	}
	if _, ok := looksLikeImagePath(txt); ok {
		t.Error("text file accepted as image")
	}
	if _, ok := looksLikeImagePath(filepath.Join(dir, "nope.png")); ok {
		t.Error("nonexistent path accepted as image")
	}
	if _, ok := looksLikeImagePath("screenshot.png"); ok {
		t.Error("bare prose word accepted as image (no such file)")
	}
}

func TestExtractImageAttachments(t *testing.T) {
	dir := t.TempDir()
	img := filepath.Join(dir, "diagram.png")
	if err := os.WriteFile(img, append(pngMagic, make([]byte, 16)...), 0o600); err != nil {
		t.Fatal(err)
	}

	t.Run("prose only is unchanged", func(t *testing.T) {
		text, blocks := extractImageAttachments("please review my screenshot.png file")
		if blocks != nil {
			t.Errorf("expected no blocks, got %d", len(blocks))
		}
		if text != "please review my screenshot.png file" {
			t.Errorf("text mutated: %q", text)
		}
	})

	t.Run("path lifted out of prose", func(t *testing.T) {
		text, blocks := extractImageAttachments("describe " + img + " please")
		if len(blocks) != 1 || blocks[0].Type != core.BlockImage || blocks[0].Image == nil {
			t.Fatalf("expected 1 image block, got %#v", blocks)
		}
		if blocks[0].Image.MimeType != "image/png" || blocks[0].Image.Data == "" {
			t.Errorf("bad image block: %#v", blocks[0].Image)
		}
		if text != "describe please" {
			t.Errorf("residual text = %q, want %q", text, "describe please")
		}
	})

	t.Run("path-only leaves empty text", func(t *testing.T) {
		text, blocks := extractImageAttachments(img)
		if len(blocks) != 1 {
			t.Fatalf("expected 1 block, got %d", len(blocks))
		}
		if text != "" {
			t.Errorf("expected empty text, got %q", text)
		}
	})
}

func TestAttachMarker(t *testing.T) {
	if m := attachMarker(nil); m != "" {
		t.Errorf("nil parts: got %q, want empty", m)
	}
	one := []core.ContentBlock{{Type: core.BlockImage, Image: &core.ImageData{}}}
	if m := attachMarker(one); m != "📎 image" {
		t.Errorf("one image: got %q", m)
	}
	two := []core.ContentBlock{{Type: core.BlockImage}, {Type: core.BlockImage}}
	if m := attachMarker(two); m != "📎 2 images" {
		t.Errorf("two images: got %q", m)
	}
}

// Package theme is the single source of truth for arbos's color palette.
// Every front-end (TUI, one-shot renderer) derives its styles from these
// colors so the product reads as one coherent surface. The default dark
// palette is sampled from Cursor's agent chat panel for visual parity; a
// light palette is selectable via Apply or the ARBOS_THEME environment
// variable.
package theme

import (
	"os"

	"github.com/charmbracelet/lipgloss"
)

// Palette is one complete set of semantic colors. Semantic names, not color
// names, so a palette swap touches only this file.
type Palette struct {
	Canvas  lipgloss.Color // terminal background
	Card    lipgloss.Color // raised surface for user messages and query bands
	Panel   lipgloss.Color // composer / follow-up input surface
	Line    lipgloss.Color // hairline highlight along the top edge of a band
	Fade    lipgloss.Color // gradient step blending a band back into the canvas
	Text    lipgloss.Color // high-contrast foreground for emphasized text
	Primary lipgloss.Color // signature green: agent identity, success, additions
	Status  lipgloss.Color // live "Working" indicator
	Accent  lipgloss.Color // links, filenames, session commands
	Command lipgloss.Color // teal highlight for resumable session commands
	Muted   lipgloss.Color // secondary metadata: version, footer, tallies
	Faint   lipgloss.Color // hints, placeholders, de-emphasized status
	Deep    lipgloss.Color // error background tone — structure and trouble
}

// Dark is the Cursor-inspired warm dark palette (the default).
var Dark = Palette{
	Canvas:  lipgloss.Color("#2c262a"),
	Card:    lipgloss.Color("#332a2f"),
	Panel:   lipgloss.Color("#312b2f"),
	Line:    lipgloss.Color("#3e373c"),
	Fade:    lipgloss.Color("#2f2a2e"),
	Text:    lipgloss.Color("#ece7ea"),
	Primary: lipgloss.Color("#87a883"),
	Status:  lipgloss.Color("#a6e22e"),
	Accent:  lipgloss.Color("#85aab3"),
	Command: lipgloss.Color("#5caeb8"),
	Muted:   lipgloss.Color("#968f94"),
	Faint:   lipgloss.Color("#6e676c"),
	Deep:    lipgloss.Color("#3e373c"),
}

// Light is the warm light counterpart, tuned for light terminal backgrounds.
var Light = Palette{
	Canvas:  lipgloss.Color("#f7f3f5"),
	Card:    lipgloss.Color("#efe8ec"),
	Panel:   lipgloss.Color("#f1eaee"),
	Line:    lipgloss.Color("#ddd4da"),
	Fade:    lipgloss.Color("#f3eef1"),
	Text:    lipgloss.Color("#2c262a"),
	Primary: lipgloss.Color("#4f7a4a"),
	Status:  lipgloss.Color("#5c8a00"),
	Accent:  lipgloss.Color("#3d6e7a"),
	Command: lipgloss.Color("#2e7f8a"),
	Muted:   lipgloss.Color("#6e676c"),
	Faint:   lipgloss.Color("#968f94"),
	Deep:    lipgloss.Color("#ecd9de"),
}

// Active semantic colors. Front-ends read these; Apply rewires them.
var (
	Canvas  = Dark.Canvas
	Card    = Dark.Card
	Panel   = Dark.Panel
	Line    = Dark.Line
	Fade    = Dark.Fade
	Text    = Dark.Text
	Primary = Dark.Primary
	Status  = Dark.Status
	Accent  = Dark.Accent
	Command = Dark.Command
	Muted   = Dark.Muted
	Faint   = Dark.Faint
	Deep    = Dark.Deep
)

func init() {
	Apply(os.Getenv("ARBOS_THEME"))
}

// Apply switches the active palette by name ("dark" or "light"). Unknown or
// empty names keep the dark default. It must run before front-end styles are
// built, since lipgloss styles capture colors at construction.
func Apply(name string) {
	p := Dark
	if name == "light" {
		p = Light
	}
	Canvas, Card, Panel, Line, Fade = p.Canvas, p.Card, p.Panel, p.Line, p.Fade
	Text, Primary, Status, Accent = p.Text, p.Primary, p.Status, p.Accent
	Command, Muted, Faint, Deep = p.Command, p.Muted, p.Faint, p.Deep
}

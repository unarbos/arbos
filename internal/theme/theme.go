// Package theme is the single source of truth for arbos's color palette.
// Every front-end (TUI, one-shot renderer) derives its styles from these five
// colors so the product reads as one coherent surface.
package theme

import "github.com/charmbracelet/lipgloss"

// The sage palette. Semantic names, not color names, so a future palette swap
// touches only this file.
var (
	// Text is the high-contrast foreground for emphasized text.
	Text = lipgloss.Color("#E6E6E6")
	// Primary is the signature sage: the agent's identity, success marks,
	// additions — anything that says "arbos did this".
	Primary = lipgloss.Color("#7B9669")
	// Accent is the pale sage used for the user's side of the conversation
	// and live tool activity.
	Accent = lipgloss.Color("#BAC8B1")
	// Muted is for everything that should recede: status lines, tallies,
	// diff context, deletions.
	Muted = lipgloss.Color("#6C8480")
	// Deep is the dark forest tone for borders and error backgrounds —
	// structure and trouble, never body text.
	Deep = lipgloss.Color("#404E3B")
)

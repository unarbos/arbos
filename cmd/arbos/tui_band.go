package main

import (
	"strings"

	"github.com/charmbracelet/lipgloss"
)

// bandLines renders a full-width single-color band: one row of vertical padding
// above and below the text body, all the same surface color — no hairline or fade.
func (r *tuiRenderer) bandLines(text string, surface lipgloss.Color) []string {
	fill := strings.Repeat(" ", r.width)
	surf := lipgloss.NewStyle().Background(surface).Width(r.width)
	body := lipgloss.NewStyle().
		Background(surface).
		Width(r.width).
		Padding(0, bandPad).
		Render(text)
	return []string{
		surf.Render(fill),
		body,
		surf.Render(fill),
	}
}

func (r *tuiRenderer) bandBodyLine(text string, surface lipgloss.Color) string {
	return lipgloss.NewStyle().
		Background(surface).
		Width(r.width).
		Padding(0, bandPad).
		Render(text)
}

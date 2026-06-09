// Command arbos-tui is a thin alias for the interactive front-end. Prefer
// running `arbos` with no arguments — same experience, one binary.
package main

import (
	"fmt"
	"os"

	"github.com/unarbos/arbos/internal/tui"
)

func main() {
	if err := tui.Run(tui.Options{}); err != nil {
		fmt.Fprintln(os.Stderr, "arbos-tui:", err)
		os.Exit(1)
	}
}

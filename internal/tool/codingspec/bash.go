package codingspec

import (
	"bytes"
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"syscall"
	"time"

	"github.com/unarbos/arbos/internal/tool"
)

// DefaultBashTimeoutSec is applied when the model omits a timeout, so an
// unattended host is not left with a hung subprocess indefinitely.
const DefaultBashTimeoutSec = 120

// BashArgs are the arguments to bash.
type BashArgs struct {
	Command string `json:"command" desc:"Bash command to execute."`
	Timeout int    `json:"timeout,omitempty" desc:"Timeout in seconds (default 120)."`
}

// bashSpec runs a shell command in the workspace root, streaming-free (the model
// sees the final result). Faithful to pi's bash tool: combined stdout+stderr,
// tail truncation to the last 2000 lines or 50KB with the full output spilled to
// a temp file, an optional timeout, full process-tree kill on timeout or
// cancel, and the verbatim status strings. Not read-only. Unix process-group
// semantics (the kernel targets Linux).
func bashSpec(root string) tool.Spec {
	return tool.NewSpec("bash",
		fmt.Sprintf("Execute a bash command in the current working directory. Returns stdout and stderr. Output is truncated to last %d lines or %dKB (whichever is hit first). If truncated, full output is saved to a temp file. Optionally provide a timeout in seconds.", DefaultMaxLines, DefaultMaxBytes/1024),
		false,
		func(ctx context.Context, a BashArgs) (string, error) {
			timeout := a.Timeout
			if timeout <= 0 {
				timeout = DefaultBashTimeoutSec
			}
			// Anchor the shell in the workspace: file tools cannot escape root via
			// Resolve, and bash starts here too. A model can still reach absolute
			// paths on the host (pi's full-privileges posture); the preamble at
			// least keeps relative paths workspace-local.
			script := fmt.Sprintf("cd %s && %s", shellSingleQuote(root), a.Command)
			cmd := exec.Command("bash", "-c", script)
			cmd.Dir = root
			cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
			var buf syncBuffer
			cmd.Stdout = &buf
			cmd.Stderr = &buf
			if err := cmd.Start(); err != nil {
				return "", fmt.Errorf("bash: %w", err)
			}
			pgid := cmd.Process.Pid
			// killIfRunning guards the process-tree kill behind reaped, set under
			// the same lock the moment cmd.Wait() returns. Once the child is
			// reaped its pgid may be reused by an unrelated process, so we must
			// never signal -pgid after that; the guard closes that race. It
			// reports whether it actually killed, so timeout/abort are recorded
			// only when a live process was signalled.
			var killMu sync.Mutex
			reaped := false
			killIfRunning := func() bool {
				killMu.Lock()
				defer killMu.Unlock()
				if reaped {
					return false
				}
				_ = syscall.Kill(-pgid, syscall.SIGKILL)
				return true
			}

			done := make(chan error, 1)
			go func() {
				err := cmd.Wait()
				killMu.Lock()
				reaped = true
				killMu.Unlock()
				done <- err
			}()

			var timedOut atomic.Bool
			timer := time.AfterFunc(time.Duration(timeout)*time.Second, func() {
				if killIfRunning() {
					timedOut.Store(true)
				}
			})
			defer timer.Stop()

			aborted := false
			var waitErr error
			select {
			case waitErr = <-done:
			case <-ctx.Done():
				aborted = killIfRunning()
				waitErr = <-done
			}
			// Stop the timer after the process is reaped so its AfterFunc cannot
			// fire against a stale pgid; the killIfRunning guard makes this safe
			// even if it already fired.
			timer.Stop()

			output := buf.String()
			withDefault := formatBashOutput(output, "(no output)")
			noDefault := formatBashOutput(output, "")

			if aborted {
				return "", fmt.Errorf("%s", appendStatus(noDefault, "Command aborted"))
			}
			if timedOut.Load() {
				return "", fmt.Errorf("%s", appendStatus(noDefault, fmt.Sprintf("Command timed out after %d seconds", timeout)))
			}
			exitCode := 0
			if waitErr != nil {
				ee, ok := waitErr.(*exec.ExitError)
				if !ok {
					return "", fmt.Errorf("bash: %w", waitErr)
				}
				exitCode = ee.ExitCode()
			}
			if exitCode != 0 {
				return "", fmt.Errorf("%s", appendStatus(withDefault, fmt.Sprintf("Command exited with code %d", exitCode)))
			}
			return withDefault, nil
		})
}

// formatBashOutput tail-truncates output, spilling the full output to a temp
// file and appending pi's notice when truncated. emptyText is used only when
// there is no output at all.
func formatBashOutput(output, emptyText string) string {
	tr := TruncateTail(output, DefaultMaxLines, DefaultMaxBytes)
	text := tr.Content
	if text == "" {
		text = emptyText
	}
	if !tr.Truncated {
		return text
	}
	spill := spillFullOutput(output)
	start := tr.TotalLines - tr.OutputLines + 1
	end := tr.TotalLines
	var notice string
	switch {
	case tr.LastLinePartial:
		notice = fmt.Sprintf("[Showing last %s of line %d (line is %s). Full output: %s]",
			FormatSize(tr.OutputBytes), end, FormatSize(lastLineBytes(output)), spill)
	case tr.TruncatedBy == "lines":
		notice = fmt.Sprintf("[Showing lines %d-%d of %d. Full output: %s]", start, end, tr.TotalLines, spill)
	default:
		notice = fmt.Sprintf("[Showing lines %d-%d of %d (%s limit). Full output: %s]", start, end, tr.TotalLines, FormatSize(DefaultMaxBytes), spill)
	}
	if text != "" {
		return text + "\n\n" + notice
	}
	return notice
}

// spillTTL bounds how long a spilled full-output file is kept. Spills live in a
// dedicated temp subdir and the just-written one is kept (the model-facing
// notice points at it), but each new spill prunes ones older than the TTL so the
// directory cannot grow without bound.
const spillTTL = 24 * time.Hour

func spillFullOutput(output string) string {
	dir := filepath.Join(os.TempDir(), "arbos-bash")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return "(unavailable)"
	}
	pruneOldSpills(dir)
	f, err := os.CreateTemp(dir, "out-*.txt")
	if err != nil {
		return "(unavailable)"
	}
	defer func() { _ = f.Close() }()
	_, _ = f.WriteString(output)
	return f.Name()
}

func pruneOldSpills(dir string) {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return
	}
	cutoff := time.Now().Add(-spillTTL)
	for _, e := range entries {
		info, err := e.Info()
		if err != nil {
			continue
		}
		if info.ModTime().Before(cutoff) {
			_ = os.Remove(filepath.Join(dir, e.Name()))
		}
	}
}

func lastLineBytes(output string) int {
	lines := splitLinesForCounting(output)
	if len(lines) == 0 {
		return 0
	}
	return len(lines[len(lines)-1])
}

func appendStatus(text, status string) string {
	if text != "" {
		return text + "\n\n" + status
	}
	return status
}

type syncBuffer struct {
	mu sync.Mutex
	b  bytes.Buffer
}

func (s *syncBuffer) Write(p []byte) (int, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.b.Write(p)
}

func (s *syncBuffer) String() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.b.String()
}

func shellSingleQuote(s string) string {
	return "'" + strings.ReplaceAll(s, "'", `'\'\''`) + "'"
}

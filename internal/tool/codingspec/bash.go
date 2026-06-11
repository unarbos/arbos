package codingspec

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"sync/atomic"
	"time"

	"github.com/unarbos/arbos/internal/tool"
)

// DefaultBashWaitSec is how long bash blocks for completion before the command
// continues as a background job. Replaces pi's kill-on-timeout default: a slow
// build is yielded to the background and kept, never lost.
const DefaultBashWaitSec = 120

// backgroundGraceWait is how long an explicitly backgrounded command is given
// to fail fast (command not found, immediate non-zero exit) before bash
// returns, so the model learns about instant failures without a second call.
const backgroundGraceWait = 500 * time.Millisecond

// BashArgs are the arguments to bash.
type BashArgs struct {
	Command string `json:"command" desc:"Bash command to execute."`
	// Description is display-only: frontends caption the command with it (the
	// terminal card header); the kernel and the executor ignore it entirely.
	Description string `json:"description,omitempty" desc:"A clear, concise description of what this command does in 5-10 words, shown to the user, e.g. \"Print current working directory\"."`
	Wait        int    `json:"wait,omitempty" desc:"Seconds to wait for completion before the command continues as a background job (default 120). The command is never killed by this; check it later with await or jobs."`
	Background  bool   `json:"background,omitempty" desc:"Return immediately and run the command as a background job (for dev servers, watchers, long builds). Overrides wait."`
	Timeout     int    `json:"timeout,omitempty" desc:"Optional hard limit in seconds: the command is killed when it expires, even after being backgrounded. Omit for no kill."`
}

// bashSpec runs a shell command in the workspace root. Every command is a
// journaled background-capable job (see job.go): combined stdout+stderr is
// written by the child itself to the job's out.log, the exit code is persisted
// by a wrapper subshell, and both survive an arbos restart. Within the wait
// window the tool behaves exactly like pi's synchronous bash (verbatim status
// strings, tail truncation); past it the command keeps running and the model
// gets a job id for await/jobs. Not read-only. Unix process-group semantics.
func bashSpec(root string, jobs *jobSupervisor, cp *checkpointer) tool.Spec {
	return tool.NewRichSpec("bash",
		fmt.Sprintf("Execute a bash command in the current working directory. Returns stdout and stderr. Output is truncated to last %d lines or %dKB (whichever is hit first); the full output is always kept in a log file. A command still running after `wait` seconds (default %d) continues as a background job — check it with await or jobs. Use background:true for dev servers and other long-lived processes.", DefaultMaxLines, DefaultMaxBytes/1024, DefaultBashWaitSec),
		false,
		func(ctx context.Context, a BashArgs) (tool.Result, error) {
			// A shell command can mutate the tree arbitrarily, so create a
			// restore point before it runs — undo then covers bash-driven changes.
			cp.ensure(ctx)
			job, done, killIfRunning, err := jobs.spawn(a.Command)
			if err != nil {
				return tool.Result{}, err
			}
			// The renderer-facing job reference (ToolResult.Details, the same
			// channel show uses for surfaces): the web UI opens the card as a
			// live terminal tab tailing the job's journal. The model never
			// sees it.
			details, _ := json.Marshal(struct {
				Job string `json:"job"`
			}{job.ID})

			var timedOut atomic.Bool
			if a.Timeout > 0 {
				// Stays armed past backgrounding on purpose: timeout is a hard
				// ceiling on the job's lifetime. killIfRunning makes a late
				// fire against a reaped pgid a no-op.
				time.AfterFunc(time.Duration(a.Timeout)*time.Second, func() {
					if killIfRunning() {
						timedOut.Store(true)
					}
				})
			}

			wait := time.Duration(a.Wait) * time.Second
			if a.Wait <= 0 {
				wait = DefaultBashWaitSec * time.Second
			}
			if a.Background {
				wait = backgroundGraceWait
			}

			aborted := false
			waitExpired := false
			select {
			case <-done:
			case <-ctx.Done():
				// An interrupt means stop: kill the job, pi's abort semantics.
				aborted = killIfRunning()
				<-done
			case <-time.After(wait):
				waitExpired = true
			}

			// A job still running when the wait expires is backgrounded; if it
			// completed in that same instant, fall through and report it.
			if waitExpired && jobs.markBackgrounded(job.ID, done) {
				return tool.Result{
					Content: jobs.withNotices(backgroundReport(jobs, job, a.Background)),
					Details: details,
				}, nil
			}

			output, skipped := jobs.readNew(job.ID, job.JournalPath())
			withDefault := formatBashOutput(output, "(no output)", job.JournalPath(), skipped)
			noDefault := formatBashOutput(output, "", job.JournalPath(), skipped)

			if aborted {
				return tool.Result{}, fmt.Errorf("%s", appendStatus(noDefault, "Command aborted"))
			}
			if timedOut.Load() {
				return tool.Result{}, fmt.Errorf("%s", appendStatus(noDefault, fmt.Sprintf("Command timed out after %d seconds", a.Timeout)))
			}
			info, err := jobs.lookup(job.ID)
			if err != nil || info.Status != JobExited {
				return tool.Result{}, fmt.Errorf("%s", appendStatus(noDefault, "Command was killed before completing"))
			}
			if info.ExitCode != 0 {
				return tool.Result{}, fmt.Errorf("%s", appendStatus(withDefault, fmt.Sprintf("Command exited with code %d", info.ExitCode)))
			}
			return tool.Result{
				Content: jobs.withNotices(withDefault, job.ID),
				Details: details,
			}, nil
		})
}

// backgroundReport tells the model a command continues as a job: the output so
// far plus everything needed to follow up (await, jobs, the pgid kill, the
// journal path).
func backgroundReport(jobs *jobSupervisor, job JobInfo, explicit bool) string {
	output, skipped := jobs.readNew(job.ID, job.JournalPath())
	text := formatBashOutput(output, "(no output yet)", job.JournalPath(), skipped)
	verb := "Command is still running"
	if explicit {
		verb = "Command is running"
	}
	status := fmt.Sprintf(
		"%s in the background as job %s (pid %d). Wait for it with await (id %q, optional regex pattern), check all jobs with jobs, or stop it with `kill -- -%d`. Full output: %s",
		verb, job.ID, job.Meta.Pid, job.ID, job.Meta.Pid, job.JournalPath())
	return appendStatus(text, status)
}

// formatBashOutput tail-truncates output for the model. The full output needs
// no spill copy: the job journal on disk already is the full record, so the
// truncation notice points there. skipped reports bytes before the in-memory
// window (huge journals); line counts are then window-relative.
func formatBashOutput(output, emptyText, journalPath string, skipped int64) string {
	tr := TruncateTail(output, DefaultMaxLines, DefaultMaxBytes)
	text := tr.Content
	if text == "" {
		text = emptyText
	}
	if !tr.Truncated && skipped == 0 {
		return text
	}
	start := tr.TotalLines - tr.OutputLines + 1
	end := tr.TotalLines
	var notice string
	switch {
	case !tr.Truncated:
		notice = fmt.Sprintf("[%s of earlier output omitted. Full output: %s]", FormatSize(int(skipped)), journalPath)
	case tr.LastLinePartial:
		notice = fmt.Sprintf("[Showing last %s of line %d (line is %s). Full output: %s]",
			FormatSize(tr.OutputBytes), end, FormatSize(lastLineBytes(output)), journalPath)
	case tr.TruncatedBy == "lines":
		notice = fmt.Sprintf("[Showing lines %d-%d of %d. Full output: %s]", start, end, tr.TotalLines, journalPath)
	default:
		notice = fmt.Sprintf("[Showing lines %d-%d of %d (%s limit). Full output: %s]", start, end, tr.TotalLines, FormatSize(DefaultMaxBytes), journalPath)
	}
	if text != "" {
		return text + "\n\n" + notice
	}
	return notice
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

func shellSingleQuote(s string) string {
	return "'" + strings.ReplaceAll(s, "'", `'\'\''`) + "'"
}

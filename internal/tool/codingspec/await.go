package codingspec

import (
	"context"
	"fmt"
	"regexp"
	"strings"
	"time"

	"github.com/unarbos/arbos/internal/tool"
)

const (
	// DefaultAwaitTimeoutSec is how long await blocks when the model omits a
	// timeout.
	DefaultAwaitTimeoutSec = 30
	// maxAwaitTimeoutSec caps a single await so one tool call cannot pin a
	// turn for more than an hour; the model just awaits again.
	maxAwaitTimeoutSec = 3600
	// awaitPollInterval is the journal/status poll cadence. Status is derived
	// from the job's files (job.go), so polling works identically for jobs
	// owned by this process and jobs re-adopted after a restart.
	awaitPollInterval = 200 * time.Millisecond
)

// AwaitArgs are the arguments to await.
type AwaitArgs struct {
	ID      string `json:"id" desc:"Background job id to wait on, e.g. 'j3' (from bash or jobs)."`
	Pattern string `json:"pattern,omitempty" desc:"Optional regex; await returns as soon as it matches the job's new output (e.g. a server-ready or error line)."`
	Timeout int    `json:"timeout,omitempty" desc:"Seconds to block before returning whatever arrived (default 30, max 3600). 0 blocks; use 1 for a quick check."`
}

// awaitSpec blocks until a background job exits, its new output matches a
// pattern, or the timeout elapses — whichever comes first — and returns the
// output produced since the model last saw the job. Read-only: it observes
// the job's journal and derived status, never the workspace.
func awaitSpec(jobs *jobSupervisor) tool.Spec {
	return tool.NewSpec("await",
		"Wait on a background job started by bash: blocks until the job exits, its new output matches an optional regex pattern, or the timeout elapses, then returns the output produced since you last saw it.",
		true,
		func(ctx context.Context, a AwaitArgs) (string, error) {
			job, err := jobs.lookup(a.ID)
			if err != nil {
				return "", err
			}
			var pattern *regexp.Regexp
			if a.Pattern != "" {
				pattern, err = regexp.Compile(a.Pattern)
				if err != nil {
					return "", fmt.Errorf("invalid pattern: %w", err)
				}
			}
			timeout := a.Timeout
			if timeout <= 0 {
				timeout = DefaultAwaitTimeoutSec
			}
			if timeout > maxAwaitTimeoutSec {
				timeout = maxAwaitTimeoutSec
			}
			deadline := time.Now().Add(time.Duration(timeout) * time.Second)

			var acc strings.Builder
			var skippedTotal int64
			collect := func() {
				text, skipped := jobs.readNew(job.ID, job.JournalPath())
				acc.WriteString(text)
				skippedTotal += skipped
			}

			status := ""
			for {
				collect()
				info, err := jobs.lookup(job.ID)
				if err != nil {
					return "", err
				}
				if info.Status != JobRunning {
					collect() // final flush raced ahead of the exit file
					status = fmt.Sprintf("Job %s %s.", job.ID, info.statusLine())
					break
				}
				if pattern != nil && pattern.MatchString(acc.String()) {
					status = fmt.Sprintf("Pattern matched. Job %s is %s.", job.ID, info.statusLine())
					break
				}
				if time.Now().After(deadline) {
					status = fmt.Sprintf("Still waiting: job %s is %s. Await again, or `kill -- -%d` to stop it.", job.ID, info.statusLine(), info.Meta.Pid)
					break
				}
				select {
				case <-ctx.Done():
					// The job is not the turn's child here; an interrupt stops
					// the waiting, never the work.
					return "", fmt.Errorf("await aborted; job %s keeps running", job.ID)
				case <-time.After(awaitPollInterval):
				}
			}

			text := formatBashOutput(acc.String(), "(no new output)", job.JournalPath(), skippedTotal)
			return jobs.withNotices(appendStatus(text, status), job.ID), nil
		})
}

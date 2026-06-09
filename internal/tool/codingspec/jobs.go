package codingspec

import (
	"context"
	"fmt"
	"strings"

	"github.com/unarbos/arbos/internal/tool"
)

// JobsArgs are the arguments to jobs (none today; the listing is always the
// whole workspace's job table).
type JobsArgs struct{}

// jobsSpec lists every background job for this workspace — including jobs
// started by a previous arbos process or a delegated child, since the listing
// is derived from the shared on-disk job directories rather than any
// in-process table. That sharing is a documented contract, not a coincidence:
// see ADR-0032.
func jobsSpec(jobs *jobSupervisor) tool.Spec {
	return tool.NewSpec("jobs",
		"List background jobs for this workspace: id, status, runtime, command, and output log path. Includes jobs that survived an arbos restart.",
		true,
		func(ctx context.Context, _ JobsArgs) (string, error) {
			infos := jobs.list()
			if len(infos) == 0 {
				return jobs.withNotices("(no background jobs)"), nil
			}
			var b strings.Builder
			listed := make([]string, 0, len(infos))
			for _, j := range infos {
				listed = append(listed, j.ID)
				cmd := j.Meta.Command
				if len(cmd) > 120 {
					cmd = cmd[:120] + "…"
				}
				fmt.Fprintf(&b, "%s: %s — `%s` — log: %s\n", j.ID, j.statusLine(), cmd, j.JournalPath())
			}
			return jobs.withNotices(strings.TrimRight(b.String(), "\n"), listed...), nil
		})
}

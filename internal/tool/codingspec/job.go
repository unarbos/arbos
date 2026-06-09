package codingspec

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"
)

// Background jobs (ADR-0032): every bash command runs as a journaled job
// whose source of truth is the filesystem, not process memory. A job owns one
// directory under the per-workspace jobs root containing exactly three files:
//
//	meta.json  — command, cwd, pid, start time; written once at spawn
//	out.log    — combined stdout+stderr; written by the child's own fds
//	exit       — "<code>\n"; written by the wrapper subshell at completion
//
// Because the child writes its own journal and its own exit code, nothing is
// lost if the arbos process dies: a restarted arbos (or a delegated child, or
// a second arbos on the same workspace) re-derives every job's status from the
// same three files. The in-process supervisor below is only a cache — live cmd
// handles for zombie reaping, per-job read offsets, and completion notices.

// jobTTL bounds how long a finished job's directory is kept. Running jobs are
// never pruned. Generous on purpose: a long-running agent may want yesterday's
// build log.
const jobTTL = 48 * time.Hour

// jobJournalWindow caps how much of a journal is loaded into memory when
// formatting a tail. Anything earlier is summarized by a byte-count notice;
// the full journal stays on disk.
const jobJournalWindow = 4 * 1024 * 1024

// JobStatus is derived, never stored: the exit file wins, then pid liveness.
type JobStatus string

const (
	JobRunning JobStatus = "running"
	JobExited  JobStatus = "exited"
	// JobKilled means the process is gone but no exit file was written — a
	// SIGKILL of the group (hard timeout, user kill) or a host reboot.
	JobKilled JobStatus = "killed"
)

// jobMeta is the once-written spawn record.
type jobMeta struct {
	Command   string    `json:"command"`
	Cwd       string    `json:"cwd"`
	Pid       int       `json:"pid"`
	StartedAt time.Time `json:"startedAt"`
}

// JobInfo is one job's derived state, assembled from its directory.
type JobInfo struct {
	ID        string
	Dir       string
	Meta      jobMeta
	Status    JobStatus
	ExitCode  int       // valid only when Status == JobExited
	EndedAt   time.Time // exit-file mtime; zero unless Status == JobExited
	JournalSz int64
}

// JournalPath returns the job's combined-output log file.
func (j JobInfo) JournalPath() string { return filepath.Join(j.Dir, "out.log") }

// jobsRoot returns the per-workspace jobs directory. Keyed by a hash of the
// workspace root so concurrent arbos processes on different workspaces do not
// see each other's jobs, while a restarted arbos on the same workspace does.
func jobsRoot(root string) string {
	sum := sha256.Sum256([]byte(root))
	return filepath.Join(os.TempDir(), "arbos-jobs", hex.EncodeToString(sum[:6]))
}

// jobSupervisor owns the in-process view of jobs: handle bookkeeping for
// reaping, read offsets for incremental output, and completion notices for
// jobs that were backgrounded. All durable state lives in the job dirs.
type jobSupervisor struct {
	root string // workspace root
	dir  string // jobs root for this workspace

	mu sync.Mutex
	// offsets tracks how much of each journal this toolset has already shown
	// the model, so bash/await return incremental output instead of repeats.
	offsets map[string]int64
	// backgrounded marks jobs this process returned to the model as "still
	// running"; their completion is worth a notice on the next tool result.
	backgrounded map[string]bool
	// notices are pending one-line completion notes keyed by job id, drained
	// by the next bash/await/jobs result. Keying lets a tool that just
	// reported a job's terminal status drop that job's now-redundant notice.
	notices []jobNotice
}

type jobNotice struct {
	id   string
	text string
}

func newJobSupervisor(root string) *jobSupervisor {
	return &jobSupervisor{
		root:         root,
		dir:          jobsRoot(root),
		offsets:      make(map[string]int64),
		backgrounded: make(map[string]bool),
	}
}

// spawn starts command as a journaled job. It returns the job's info, a
// channel closed when the process is reaped, and a kill function that
// SIGKILLs the job's whole process group — but only while the child is
// un-reaped, because after reaping the pgid may belong to an unrelated
// process. The wrapper subshell isolates `exit` statements in the command and
// persists the exit code itself, so the code survives this process dying
// before the job does.
func (s *jobSupervisor) spawn(command string) (JobInfo, <-chan struct{}, func() bool, error) {
	if err := os.MkdirAll(s.dir, 0o755); err != nil {
		return JobInfo{}, nil, nil, fmt.Errorf("jobs dir: %w", err)
	}
	s.pruneFinished()
	id, dir, err := allocJobDir(s.dir)
	if err != nil {
		return JobInfo{}, nil, nil, err
	}
	journal, err := os.OpenFile(filepath.Join(dir, "out.log"), os.O_CREATE|os.O_WRONLY, 0o644)
	if err != nil {
		return JobInfo{}, nil, nil, fmt.Errorf("job journal: %w", err)
	}
	exitPath := filepath.Join(dir, "exit")
	script := fmt.Sprintf("( cd %s && %s\n); echo $? > %s",
		shellSingleQuote(s.root), command, shellSingleQuote(exitPath))
	cmd := exec.Command("bash", "-c", script)
	cmd.Dir = s.root
	cmd.Stdout = journal
	cmd.Stderr = journal
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	if err := cmd.Start(); err != nil {
		_ = journal.Close()
		_ = os.RemoveAll(dir)
		return JobInfo{}, nil, nil, fmt.Errorf("bash: %w", err)
	}
	// The child holds its own dup of the journal fd; ours is no longer needed.
	_ = journal.Close()

	meta := jobMeta{Command: command, Cwd: s.root, Pid: cmd.Process.Pid, StartedAt: time.Now()}
	if b, err := json.Marshal(meta); err == nil {
		_ = os.WriteFile(filepath.Join(dir, "meta.json"), b, 0o644)
	}

	// killIfRunning guards the group kill behind reaped, set under the same
	// lock the moment cmd.Wait() returns (pi's race guard, preserved). It
	// reports whether a live process was actually signalled.
	pgid := cmd.Process.Pid
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

	done := make(chan struct{})
	go func() {
		_ = cmd.Wait() // reap; the exit file is the durable record
		killMu.Lock()
		reaped = true
		killMu.Unlock()
		close(done)
		s.mu.Lock()
		if s.backgrounded[id] {
			delete(s.backgrounded, id)
			if info, err := loadJob(dir); err == nil {
				s.notices = append(s.notices, jobNotice{id: id, text: fmt.Sprintf("[background job %s finished: %s]", id, info.statusLine())})
			}
		}
		s.mu.Unlock()
	}()

	return JobInfo{ID: id, Dir: dir, Meta: meta, Status: JobRunning}, done, killIfRunning, nil
}

// markBackgrounded records that the model was told this job continues in the
// background, arming a completion notice. Reports false when the job already
// finished (the caller should report final status instead).
func (s *jobSupervisor) markBackgrounded(id string, done <-chan struct{}) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	select {
	case <-done:
		return false
	default:
	}
	s.backgrounded[id] = true
	return true
}

// drainNotices returns and clears pending completion notices, skipping the
// ids in except (jobs whose terminal status the caller just reported itself).
// Appended to the next bash/await/jobs result so a backgrounded job's
// completion reaches the model without any engine-level push channel.
func (s *jobSupervisor) drainNotices(except ...string) string {
	s.mu.Lock()
	defer s.mu.Unlock()
	if len(s.notices) == 0 {
		return ""
	}
	skip := map[string]bool{}
	for _, id := range except {
		skip[id] = true
	}
	var lines []string
	for _, n := range s.notices {
		if !skip[n.id] {
			lines = append(lines, n.text)
		}
	}
	s.notices = nil
	return strings.Join(lines, "\n")
}

// withNotices appends pending completion notices to a tool result, minus any
// for the excepted job ids.
func (s *jobSupervisor) withNotices(text string, except ...string) string {
	n := s.drainNotices(except...)
	if n == "" {
		return text
	}
	if text == "" {
		return n
	}
	return text + "\n\n" + n
}

// lookup loads one job by id from the jobs root.
func (s *jobSupervisor) lookup(id string) (JobInfo, error) {
	info, err := loadJob(filepath.Join(s.dir, id))
	if err != nil {
		return JobInfo{}, fmt.Errorf("no such job %q (use jobs to list)", id)
	}
	return info, nil
}

// list loads every job under the jobs root, oldest first.
func (s *jobSupervisor) list() []JobInfo {
	return listJobs(s.dir)
}

// listJobs loads every job under a jobs root, oldest first. Package-level on
// purpose: the listing is derived purely from the shared on-disk directories,
// so any reader (a tool, the turn-start context injector, a future inspector)
// sees the same table with no supervisor handle.
func listJobs(dir string) []JobInfo {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil
	}
	var out []JobInfo
	for _, e := range entries {
		if !e.IsDir() {
			continue
		}
		if info, err := loadJob(filepath.Join(dir, e.Name())); err == nil {
			out = append(out, info)
		}
	}
	sort.Slice(out, func(i, j int) bool { return jobNum(out[i].ID) < jobNum(out[j].ID) })
	return out
}

// readNew returns journal output beyond what this toolset has already shown
// for the job, advancing the stored offset. Output older than jobJournalWindow
// behind the end is skipped (reported via skipped) so a gigabyte journal never
// lands in memory.
func (s *jobSupervisor) readNew(id, journalPath string) (text string, skipped int64) {
	s.mu.Lock()
	offset := s.offsets[id]
	s.mu.Unlock()

	f, err := os.Open(journalPath)
	if err != nil {
		return "", 0
	}
	defer func() { _ = f.Close() }()
	st, err := f.Stat()
	if err != nil || st.Size() <= offset {
		return "", 0
	}
	size := st.Size()
	start := offset
	if size-start > jobJournalWindow {
		skipped = size - start - jobJournalWindow
		start = size - jobJournalWindow
	}
	buf := make([]byte, size-start)
	n, _ := f.ReadAt(buf, start)
	s.mu.Lock()
	if s.offsets[id] < start+int64(n) {
		s.offsets[id] = start + int64(n)
	}
	s.mu.Unlock()
	return string(buf[:n]), skipped
}

// pruneFinished removes finished job dirs older than jobTTL. Running jobs are
// left alone no matter their age.
func (s *jobSupervisor) pruneFinished() {
	entries, err := os.ReadDir(s.dir)
	if err != nil {
		return
	}
	cutoff := time.Now().Add(-jobTTL)
	for _, e := range entries {
		dir := filepath.Join(s.dir, e.Name())
		info, err := loadJob(dir)
		if err != nil {
			// Unreadable leftovers (crash between mkdir and meta write): prune
			// by directory mtime.
			if fi, statErr := e.Info(); statErr == nil && fi.ModTime().Before(cutoff) {
				_ = os.RemoveAll(dir)
			}
			continue
		}
		if info.Status != JobRunning && info.Meta.StartedAt.Before(cutoff) {
			_ = os.RemoveAll(dir)
		}
	}
}

// allocJobDir claims the next free short id (j1, j2, …) with an exclusive
// Mkdir, so concurrent arbos processes sharing a workspace cannot collide.
func allocJobDir(root string) (string, string, error) {
	next := 1
	if entries, err := os.ReadDir(root); err == nil {
		for _, e := range entries {
			if n := jobNum(e.Name()); n >= next {
				next = n + 1
			}
		}
	}
	for i := 0; i < 100; i++ {
		id := "j" + strconv.Itoa(next)
		dir := filepath.Join(root, id)
		err := os.Mkdir(dir, 0o755)
		if err == nil {
			return id, dir, nil
		}
		if !os.IsExist(err) {
			return "", "", fmt.Errorf("job dir: %w", err)
		}
		next++
	}
	return "", "", fmt.Errorf("job dir: could not allocate an id")
}

func jobNum(id string) int {
	if !strings.HasPrefix(id, "j") {
		return -1
	}
	n, err := strconv.Atoi(id[1:])
	if err != nil {
		return -1
	}
	return n
}

// loadJob derives a job's current state from its directory: meta.json is the
// identity, the exit file wins over everything, and otherwise pid liveness
// decides running vs killed.
func loadJob(dir string) (JobInfo, error) {
	b, err := os.ReadFile(filepath.Join(dir, "meta.json"))
	if err != nil {
		return JobInfo{}, err
	}
	var meta jobMeta
	if err := json.Unmarshal(b, &meta); err != nil {
		return JobInfo{}, err
	}
	info := JobInfo{ID: filepath.Base(dir), Dir: dir, Meta: meta}
	if st, err := os.Stat(info.JournalPath()); err == nil {
		info.JournalSz = st.Size()
	}
	exitPath := filepath.Join(dir, "exit")
	if eb, err := os.ReadFile(exitPath); err == nil {
		code, convErr := strconv.Atoi(strings.TrimSpace(string(eb)))
		if convErr == nil {
			info.Status = JobExited
			info.ExitCode = code
			if st, statErr := os.Stat(exitPath); statErr == nil {
				info.EndedAt = st.ModTime()
			}
			return info, nil
		}
	}
	if pidAlive(meta.Pid) {
		info.Status = JobRunning
	} else {
		info.Status = JobKilled
	}
	return info, nil
}

// pidAlive reports whether pid still exists. Signal 0 probes without sending;
// EPERM still means "exists". A reaped child or reused-then-exited pid reads
// as dead, which is the honest answer for a job with no exit file.
func pidAlive(pid int) bool {
	if pid <= 0 {
		return false
	}
	err := syscall.Kill(pid, 0)
	return err == nil || err == syscall.EPERM
}

// NoJobsContext is JobsContext's rendering of an empty job table. Exported so
// the host's injector can tell "nothing to show" from a real table and apply
// its only-append-when-changed discipline (ADR-0032).
const NoJobsContext = "(no background jobs)"

// recentFinishedWindow bounds how long a finished job stays in the injected
// job table. Finished jobs age out of the turn-start context here; the jobs
// tool (and the 48h jobTTL) still shows them on demand.
const recentFinishedWindow = 10 * time.Minute

// JobsContext renders the workspace's background-job table for turn-start
// context injection (ADR-0032): every running job, plus jobs that finished
// recently enough that the model may still be acting on them. Stateless and
// derived from the shared job directories, so it reflects jobs started by a
// previous arbos process or a delegated child exactly like the jobs tool.
//
// The rendering is deliberately duration-free (contextLine): it changes only
// when the table itself changes, so re-injection (and the prefix cache) churns
// on real state transitions, not on the passage of time.
func JobsContext(root string) string {
	cutoff := time.Now().Add(-recentFinishedWindow)
	var lines []string
	for _, j := range listJobs(jobsRoot(root)) {
		if j.Status != JobRunning {
			ended := j.EndedAt
			if ended.IsZero() {
				ended = j.Meta.StartedAt // killed jobs record no end; age by start
			}
			if ended.Before(cutoff) {
				continue
			}
		}
		cmd := j.Meta.Command
		if len(cmd) > 80 {
			cmd = cmd[:80] + "…"
		}
		lines = append(lines, fmt.Sprintf("%s: %s — `%s`", j.ID, j.contextLine(), cmd))
	}
	if len(lines) == 0 {
		return NoJobsContext
	}
	return "Background jobs in this workspace (follow with await, inspect with jobs):\n" + strings.Join(lines, "\n")
}

// RunWorkspaceCmd runs command as a journaled job in root's workspace and
// waits for it — the plan scheduler's mechanical executor. The run is visible
// in the job table like any other job (the model's injected table, the jobs
// tool, the front-door brief), so a kernel-run pipeline is inspectable with
// the same eyes as everything else. Cancellation kills the job's process
// group. Returns the job id, exit code (-1 when killed), and the journal's
// tail.
func RunWorkspaceCmd(ctx context.Context, root, command string) (jobID string, exitCode int, tail string, err error) {
	s := newJobSupervisor(root)
	info, done, kill, err := s.spawn(command)
	if err != nil {
		return "", -1, "", err
	}
	select {
	case <-done:
	case <-ctx.Done():
		kill()
		<-done
	}
	final, err := loadJob(info.Dir)
	if err != nil {
		return info.ID, -1, "", fmt.Errorf("job %s: %w", info.ID, err)
	}
	code := final.ExitCode
	if final.Status == JobKilled {
		code = -1
	}
	return info.ID, code, journalTail(info.Dir, 4096), nil
}

// journalTail reads the last max bytes of a job's output log, trimmed.
func journalTail(dir string, max int64) string {
	f, err := os.Open(filepath.Join(dir, "out.log"))
	if err != nil {
		return ""
	}
	defer func() { _ = f.Close() }()
	st, err := f.Stat()
	if err != nil {
		return ""
	}
	off := int64(0)
	if st.Size() > max {
		off = st.Size() - max
	}
	buf := make([]byte, st.Size()-off)
	if _, err := f.ReadAt(buf, off); err != nil {
		return ""
	}
	return strings.TrimSpace(string(buf))
}

// JobsBrief renders the HUMAN front-door view of the job table, as JobsContext
// renders the model's. Only jobs actually running appear — finished jobs are
// history, not a greeting — one line each: id, age, and the first line of the
// command, clipped. Returns "" when nothing is running so the caller prints no
// section at all.
func JobsBrief(root string) string {
	var lines []string
	for _, j := range listJobs(jobsRoot(root)) {
		if j.Status != JobRunning {
			continue
		}
		cmd := j.Meta.Command
		if i := strings.IndexByte(cmd, '\n'); i >= 0 {
			cmd = cmd[:i] + " …"
		}
		if len(cmd) > 64 {
			cmd = cmd[:64] + "…"
		}
		age := time.Since(j.Meta.StartedAt).Round(time.Second)
		lines = append(lines, fmt.Sprintf("  ▸ %s %s · %s", j.ID, cmd, age))
	}
	if len(lines) == 0 {
		return ""
	}
	return "running:\n" + strings.Join(lines, "\n")
}

// contextLine is the duration-free sibling of statusLine, used by JobsContext
// so the injected table is stable while a job's state is unchanged.
func (j JobInfo) contextLine() string {
	switch j.Status {
	case JobExited:
		return fmt.Sprintf("exited with code %d", j.ExitCode)
	case JobKilled:
		return "killed (no exit recorded)"
	default:
		return fmt.Sprintf("running (pid %d)", j.Meta.Pid)
	}
}

// statusLine renders a job's state the way every tool reports it.
func (j JobInfo) statusLine() string {
	switch j.Status {
	case JobExited:
		if !j.EndedAt.IsZero() {
			return fmt.Sprintf("exited with code %d after %s", j.ExitCode, j.EndedAt.Sub(j.Meta.StartedAt).Round(time.Second))
		}
		return fmt.Sprintf("exited with code %d", j.ExitCode)
	case JobKilled:
		return "killed (no exit recorded)"
	default:
		return fmt.Sprintf("running for %s (pid %d)", time.Since(j.Meta.StartedAt).Round(time.Second), j.Meta.Pid)
	}
}

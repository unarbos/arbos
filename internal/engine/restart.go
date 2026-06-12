package engine

import (
	"context"
	"os"
	"syscall"
	"time"
)

// RestartConfig tunes the self-restart watcher. Zero values get sane defaults
// from WatchRestart.
type RestartConfig struct {
	// Binary is the executable to watch and exec into. Empty = the current
	// executable (os.Executable), which is what a rebuild overwrites in place.
	Binary string
	// Poll is how often to stat the binary and check the in-flight count.
	Poll time.Duration
	// Settle is how long the on-disk binary must hold still before it is
	// trusted. `go build -o` truncates and rewrites the output (not an atomic
	// rename), so a stat that just changed may be a half-written file; only a
	// stat unchanged for a full settle window is a finished build.
	Settle time.Duration
	// Logf, when set, reports restart decisions. nil = silent.
	Logf func(format string, args ...any)
}

// WatchRestart makes arbos safe to rebuild in place while it runs. It watches
// the server's own executable until ctx is cancelled; when the file on disk no
// longer matches the binary this process booted from — a dev-loop rebuild, an
// upgrade, or arbos editing its own source and running `go build` — it
// re-execs the new binary, but ONLY at an idle turn boundary (InFlight()==0).
// A turn that rebuilds arbos's own binary can therefore never be decapitated
// by the rebuild it triggered: the swap waits for the turn to finish.
//
// The contract is deliberately external-tooling-free: replace the file at the
// binary's own path however you like and the running process swaps itself. No
// sentinel files, no signals, no watcher process.
//
// On a swap it syscall.Exec's the new binary in place — same PID, same
// argv/env — a clean hot reload: durable session state lives in the store and
// the UI reconnects its seam, so an idle swap is invisible. If exec fails
// (e.g. the file is corrupt), the process keeps serving the old binary and
// waits for the next replacement.
func (e *Engine) WatchRestart(ctx context.Context, cfg RestartConfig) {
	if cfg.Poll <= 0 {
		cfg.Poll = 500 * time.Millisecond
	}
	if cfg.Settle <= 0 {
		cfg.Settle = 2 * time.Second
	}
	logf := cfg.Logf
	if logf == nil {
		logf = func(string, ...any) {}
	}
	bin := cfg.Binary
	if bin == "" {
		// Resolve once, now, while the path still names the running image. On
		// Linux /proc/self/exe decays to "... (deleted)" after a replacement,
		// so a late resolve would watch a path that no longer exists.
		exe, err := os.Executable()
		if err != nil {
			logf("restart: cannot resolve own executable, self-restart disabled: %v", err)
			return
		}
		bin = exe
	}

	// baseline identifies the build this process is running. "Restart pending"
	// means the file at bin no longer matches it.
	baseline, err := os.Stat(bin)
	if err != nil {
		logf("restart: cannot stat %s, self-restart disabled: %v", bin, err)
		return
	}
	sameBuild := func(a, b os.FileInfo) bool {
		return os.SameFile(a, b) && a.ModTime().Equal(b.ModTime()) && a.Size() == b.Size()
	}

	var (
		candidate os.FileInfo // last stat that differed from baseline
		stableAt  time.Time   // when candidate was first observed
		failed    os.FileInfo // a build that failed to exec; ignored until replaced again
	)
	t := time.NewTicker(cfg.Poll)
	defer t.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-t.C:
		}
		// A requested restart (configuration change) takes the same idle-only
		// exec as a binary swap, just without waiting for the file to differ.
		if e.restartReq.Load() && e.InFlight() == 0 {
			logf("restart: configuration change and all sessions idle — re-execing %s", bin)
			if err := syscall.Exec(bin, os.Args, os.Environ()); err != nil {
				logf("restart: exec failed, staying on current binary: %v", err)
				e.restartReq.Store(false) // don't spin on a broken exec
			}
			continue
		}
		cur, err := os.Stat(bin)
		if err != nil {
			candidate = nil // mid-replacement (rm before cp); wait for it to reappear
			continue
		}
		if sameBuild(baseline, cur) {
			candidate = nil // unchanged, or rolled back: nothing pending
			continue
		}
		if failed != nil && sameBuild(failed, cur) {
			continue // this exact build already failed to exec; wait for a newer one
		}
		if candidate == nil || !sameBuild(candidate, cur) {
			candidate, stableAt = cur, time.Now()
			continue // file is (possibly) still being written: restart the settle clock
		}
		if time.Since(stableAt) < cfg.Settle {
			continue
		}
		if e.InFlight() > 0 {
			continue // restart stays pending; swap at the next idle poll
		}
		logf("restart: binary replaced and all sessions idle — re-execing %s", bin)
		if err := syscall.Exec(bin, os.Args, os.Environ()); err != nil {
			// Exec replaces the image on success and never returns; reaching
			// here means it failed. Keep serving the old binary.
			logf("restart: exec failed, staying on current binary: %v", err)
			failed = cur
			candidate = nil
		}
	}
}

package engine

import (
	"context"
	"os"
	"syscall"
	"time"
)

// RestartConfig tunes the graceful-restart supervisor. Zero values get sane
// defaults from WatchRestart.
type RestartConfig struct {
	// Sentinel is the path a rebuild touches to request a restart. The dev loop
	// (scripts/dev.sh) writes it only after a successful build, so its presence
	// means "a newer binary is on disk, swap to it when safe".
	Sentinel string
	// Binary is the executable to exec into. Empty = the current executable
	// (os.Executable), which is what the dev loop overwrites in place.
	Binary string
	// Poll is how often to check the sentinel and the in-flight count. The
	// restart waits for both "sentinel exists" and "no turn in flight", so a
	// short period keeps the swap prompt without busy-spinning.
	Poll time.Duration
	// Logf, when set, reports the restart decision. nil = silent.
	Logf func(format string, args ...any)
}

// WatchRestart runs a graceful-restart supervisor until ctx is cancelled. It
// is the engine half of the self-edit-safe dev loop: the build writes a
// sentinel instead of killing the server, and this watcher swaps to the new
// binary ONLY at a turn boundary (InFlight()==0), so a turn that edits arbos's
// own source can never be decapitated by the rebuild it triggers.
//
// On a swap it syscall.Exec's the new binary in place — same PID, same
// argv/env — which is a clean hot reload: durable session state lives in the
// store and the UI reconnects its seam, so an idle swap is invisible. If exec
// fails the process is left running the old binary (the swap is best-effort,
// never fatal).
func (e *Engine) WatchRestart(ctx context.Context, cfg RestartConfig) {
	if cfg.Sentinel == "" {
		return // no sentinel configured: nothing to watch
	}
	if cfg.Poll <= 0 {
		cfg.Poll = 500 * time.Millisecond
	}
	bin := cfg.Binary
	if bin == "" {
		exe, err := os.Executable()
		if err != nil {
			if cfg.Logf != nil {
				cfg.Logf("restart: cannot resolve own executable, watcher disabled: %v", err)
			}
			return
		}
		bin = exe
	}

	t := time.NewTicker(cfg.Poll)
	defer t.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-t.C:
			if _, err := os.Stat(cfg.Sentinel); err != nil {
				continue // no restart pending
			}
			// A restart is pending. Only swap when no turn is executing, so we
			// never drop one. While turns keep arriving we simply keep waiting;
			// the sentinel stays until we act on it.
			if e.InFlight() > 0 {
				continue
			}
			// Consume the sentinel before exec so a failed/again-pending build
			// must re-touch it, and a successful exec doesn't loop the new
			// process straight back into a restart.
			_ = os.Remove(cfg.Sentinel)
			if cfg.Logf != nil {
				cfg.Logf("restart: idle and restart pending — re-execing %s", bin)
			}
			if err := syscall.Exec(bin, os.Args, os.Environ()); err != nil {
				// Exec replaces the image on success and never returns; reaching
				// here means it failed. Keep serving the old binary.
				if cfg.Logf != nil {
					cfg.Logf("restart: exec failed, staying on current binary: %v", err)
				}
			}
		}
	}
}

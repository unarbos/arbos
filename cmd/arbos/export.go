package main

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/piwire"
	"github.com/unarbos/arbos/internal/ports"
	"github.com/unarbos/arbos/internal/sqlite"
	"github.com/unarbos/arbos/internal/trajectory"
)

func runExport(cfg cliConfig, args []string) error {
	fs := flag.NewFlagSet("export", flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	var all, messages bool
	fs.BoolVar(&all, "all", false, "")
	fs.BoolVar(&messages, "messages", false, "")
	if err := fs.Parse(normalizeLongFlags(args)); err != nil {
		return fmt.Errorf("export: %w", err)
	}
	ids := fs.Args()
	if all && len(ids) > 0 {
		return errors.New("export: pass session ids or --all, not both")
	}
	if !all && len(ids) == 0 {
		return errors.New("export: need at least one session id, or --all (see `arbos ls`)")
	}

	store, err := piwire.OpenStore(cfg.db)
	if err != nil {
		return fmt.Errorf("open store: %w", err)
	}
	defer func() { _ = store.Close() }()

	ctx := context.Background()
	sids := make([]core.SessionID, 0, len(ids))
	if all {
		if sids, err = store.SessionIDs(ctx); err != nil {
			return err
		}
	} else {
		for _, id := range ids {
			if _, err := store.Get(ctx, core.SessionID(id)); err != nil {
				if errors.Is(err, ports.ErrSessionNotFound) {
					return fmt.Errorf("export: unknown session %q (see `arbos ls`)", id)
				}
				return err
			}
			sids = append(sids, core.SessionID(id))
		}
	}

	enc := trajectory.NewEncoder(os.Stdout)
	for _, id := range sids {
		if err := exportSession(ctx, store, id, messages, enc); err != nil {
			return err
		}
	}
	return nil
}

func exportSession(ctx context.Context, store *sqlite.Store, id core.SessionID, messages bool, enc *json.Encoder) error {
	events, err := store.Events(ctx, id)
	if err != nil {
		return fmt.Errorf("export %s: %w", id, err)
	}
	if messages {
		// A session predating config logging falls back to the session row's
		// model rather than guessing from the current binary's templates.
		var fallback string
		if sess, err := store.Get(ctx, id); err == nil {
			fallback = sess.Model
		}
		return trajectory.WriteMessages(enc, id, events, fallback)
	}
	return trajectory.WriteEvents(enc, id, events)
}

package main

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/piwire"
	"github.com/unarbos/arbos/internal/ports"
	"github.com/unarbos/arbos/internal/sqlite"
)

// eventLine is one full-fidelity JSONL record: the complete persisted event
// with its payload verbatim (reasoning, tool calls, usage, provider meta,
// config — everything the store holds). This is the lossless trajectory shape;
// --messages renders the same log as a flat provider-facing conversation.
type eventLine struct {
	SessionID string          `json:"session_id"`
	Seq       int64           `json:"seq"`
	TurnID    int64           `json:"turn_id,omitempty"`
	Kind      string          `json:"kind"`
	Version   int             `json:"version"`
	CreatedAt time.Time       `json:"created_at"`
	Payload   json.RawMessage `json:"payload"`
}

// messagesLine is one session rendered through core.Project — the exact
// conversation the provider saw, system prompt and injected context included
// (read from the log's config events, not the current binary). One line per
// session, training-sample shaped.
type messagesLine struct {
	SessionID string            `json:"session_id"`
	Model     string            `json:"model,omitempty"`
	Tools     []core.ToolSchema `json:"tools,omitempty"`
	Messages  []core.Message    `json:"messages"`
}

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

	enc := json.NewEncoder(os.Stdout)
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
		line := messagesLine{SessionID: string(id)}
		// The config events in the log are the authority for what the model
		// saw; a session predating config logging falls back to the session
		// row's model and projects without a system prompt rather than
		// guessing from the current binary's templates.
		turnCfg, ok := core.LatestConfig(events)
		if ok {
			line.Model = turnCfg.Model
			line.Tools = turnCfg.Tools
		} else if sess, err := store.Get(ctx, id); err == nil {
			line.Model = sess.Model
		}
		line.Messages = core.Project(events, turnCfg.SystemPrompt)
		return enc.Encode(line)
	}

	for i := range events {
		payload, err := core.EncodePayload(events[i].Payload)
		if err != nil {
			return fmt.Errorf("export %s: encode seq %d: %w", id, events[i].Seq, err)
		}
		err = enc.Encode(eventLine{
			SessionID: string(id),
			Seq:       events[i].Seq,
			TurnID:    events[i].TurnID,
			Kind:      string(events[i].Payload.Kind()),
			Version:   events[i].Version,
			CreatedAt: events[i].CreatedAt.UTC(),
			Payload:   payload,
		})
		if err != nil {
			return err
		}
	}
	return nil
}

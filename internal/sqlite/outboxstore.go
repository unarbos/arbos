package sqlite

import (
	"context"
	"fmt"
	"time"

	"github.com/unarbos/arbos/internal/outbox"
)

// This file is the storage half of the agent's voice (the "outbox"):
// undelivered messages to the user, claimed atomically by whichever door
// reaches them first. Delivery semantics live in internal/outbox; this is
// dumb rows, like the atom and plan files.

// Notify appends one undelivered message for the user.
func (s *Store) Notify(ctx context.Context, text, session string) error {
	s.writeMu.Lock()
	defer s.writeMu.Unlock()
	_, err := s.db.ExecContext(ctx,
		`INSERT INTO outbox (message, session_id, created_at) VALUES (?, ?, ?)`,
		text, session, time.Now().UnixNano())
	if err != nil {
		return fmt.Errorf("notify: %w", err)
	}
	return nil
}

// ClaimOutbox atomically claims every undelivered message for one door and
// returns them oldest first. Claim-then-deliver inside one transaction under
// the write lock: doors racing across processes serialize on SQLite's write
// lock, so a message is only ever returned to one caller.
func (s *Store) ClaimOutbox(ctx context.Context, via string) ([]outbox.Message, error) {
	s.writeMu.Lock()
	defer s.writeMu.Unlock()

	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return nil, fmt.Errorf("claim outbox: begin: %w", err)
	}
	defer func() { _ = tx.Rollback() }()

	rows, err := tx.QueryContext(ctx,
		`SELECT id, message, session_id, created_at FROM outbox
		  WHERE delivered_at = 0 ORDER BY id`)
	if err != nil {
		return nil, fmt.Errorf("claim outbox: select: %w", err)
	}
	var msgs []outbox.Message
	for rows.Next() {
		var (
			m       outbox.Message
			created int64
		)
		if err := rows.Scan(&m.ID, &m.Text, &m.Session, &created); err != nil {
			_ = rows.Close()
			return nil, fmt.Errorf("claim outbox: scan: %w", err)
		}
		m.CreatedAt = time.Unix(0, created).UTC()
		msgs = append(msgs, m)
	}
	if err := rows.Close(); err != nil {
		return nil, fmt.Errorf("claim outbox: %w", err)
	}
	if len(msgs) == 0 {
		return nil, nil
	}

	now := time.Now().UnixNano()
	for _, m := range msgs {
		if _, err := tx.ExecContext(ctx,
			`UPDATE outbox SET delivered_at = ?, delivered_via = ? WHERE id = ? AND delivered_at = 0`,
			now, via, m.ID); err != nil {
			return nil, fmt.Errorf("claim outbox: claim %d: %w", m.ID, err)
		}
	}
	if err := tx.Commit(); err != nil {
		return nil, fmt.Errorf("claim outbox: commit: %w", err)
	}
	return msgs, nil
}

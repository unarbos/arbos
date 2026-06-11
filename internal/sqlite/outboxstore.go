package sqlite

import (
	"context"
	"fmt"
	"strings"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/outbox"
)

// This file is the storage half of the agent's voice (the "outbox"):
// undelivered messages to the user, claimed atomically by whichever door
// reaches them first. Delivery semantics live in internal/outbox; this is
// dumb rows, like the atom and plan files.

// Notify appends one undelivered message for the user, stamped by the session
// that spoke. Sugar for NotifyFrom with no source (never coalesced).
func (s *Store) Notify(ctx context.Context, text, session string) error {
	return s.NotifyFrom(ctx, text, session, "")
}

// ownerChainMax bounds the spawn-chain walk; deeper nesting than this is a
// cycle or a bug, and the message simply stays addressed where the walk stops.
const ownerChainMax = 8

// NotifyFrom appends one undelivered message. The message is ADDRESSED to a
// principal and PREFERS the conversation it came from:
//
//   - the stamping session's owner chain (chat → wake → delegate …) resolves
//     to its root conversation, so machine-spawned work speaks into the chat
//     that created it, however deep the nesting;
//   - the principal rides along as the durable address — delivery may fall
//     back to any of the principal's doors, but never to another principal.
//
// A non-empty source (e.g. "node:12") makes the message COALESCING: a new
// firing supersedes its own undelivered predecessor, so a recurring ping
// never piles up while no door is open — the next door gets the latest
// reading, not the backlog. The same latest-per-source rule the projection
// applies to injected context (core.ContextPayload).
func (s *Store) NotifyFrom(ctx context.Context, text, session, source string) error {
	principal := ""
	cur := session
	for hop := 0; cur != "" && hop < ownerChainMax; hop++ {
		var owner, p, spawnedBy string
		if err := s.db.QueryRowContext(ctx,
			`SELECT owner, principal, spawned_by FROM sessions WHERE id = ?`, cur).
			Scan(&owner, &p, &spawnedBy); err != nil {
			break // unknown session: keep the current address as-is
		}
		// A recurring node's wake speaking through the notify tool is that
		// node's voice: inherit the node as the coalescing source so its
		// pings supersede each other exactly like a mechanical notify node's.
		if hop == 0 && source == "" {
			if _, ok := core.ParseSpawnedByNode(spawnedBy); ok {
				source = spawnedBy
			}
		}
		if p != "" {
			principal = p
		}
		if owner == "" {
			break // reached the root conversation
		}
		session, cur = owner, owner
	}
	if principal == "" {
		principal = core.PrincipalLocal
	}

	s.writeMu.Lock()
	defer s.writeMu.Unlock()
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return fmt.Errorf("notify: begin: %w", err)
	}
	defer func() { _ = tx.Rollback() }()
	if source != "" {
		if _, err := tx.ExecContext(ctx,
			`DELETE FROM outbox WHERE source = ? AND delivered_at = 0`, source); err != nil {
			return fmt.Errorf("notify: supersede %q: %w", source, err)
		}
	}
	if _, err := tx.ExecContext(ctx,
		`INSERT INTO outbox (message, session_id, principal, source, created_at) VALUES (?, ?, ?, ?, ?)`,
		text, session, principal, source, time.Now().UnixNano()); err != nil {
		return fmt.Errorf("notify: %w", err)
	}
	if err := tx.Commit(); err != nil {
		return fmt.Errorf("notify: commit: %w", err)
	}
	return nil
}

// ClaimOutbox atomically claims every undelivered message for one door and
// returns them oldest first. Claim-then-deliver inside one transaction under
// the write lock: doors racing across processes serialize on SQLite's write
// lock, so a message is only ever returned to one caller.
func (s *Store) ClaimOutbox(ctx context.Context, via string) ([]outbox.Message, error) {
	return s.claimOutbox(ctx, via, "", nil, time.Time{})
}

// ClaimOutboxFor is the addressed claim: it takes the principal's messages —
// never another principal's — preferring each message's own conversation:
//
//   - messages whose preferred session is in sessions (that chat has this
//     door open) deliver now;
//   - broadcast-class messages (no session, or the kernel's) deliver to any
//     door;
//   - messages older than staleBefore deliver to any of the principal's
//     doors: within the grace window a chat's voice waits for that chat,
//     past it being heard somewhere beats waiting for a conversation that
//     may never reopen.
//
// Pass a zero staleBefore to disable the fallback.
func (s *Store) ClaimOutboxFor(ctx context.Context, via, principal string, sessions []string, staleBefore time.Time) ([]outbox.Message, error) {
	if sessions == nil {
		sessions = []string{}
	}
	return s.claimOutbox(ctx, via, principal, sessions, staleBefore)
}

func (s *Store) claimOutbox(ctx context.Context, via, principal string, sessions []string, staleBefore time.Time) ([]outbox.Message, error) {
	s.writeMu.Lock()
	defer s.writeMu.Unlock()

	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return nil, fmt.Errorf("claim outbox: begin: %w", err)
	}
	defer func() { _ = tx.Rollback() }()

	query := `SELECT id, message, session_id, created_at FROM outbox
		  WHERE delivered_at = 0`
	var args []any
	if sessions != nil {
		// Address first: only this principal's mail (rows from before the
		// principal column exist as '' and stay claimable by any door).
		query += ` AND principal IN (?, '')`
		args = append(args, principal)
		scoped := append(append([]string{}, outbox.BroadcastSessions...), sessions...)
		query += ` AND (session_id IN (?` + strings.Repeat(",?", len(scoped)-1) + `)`
		for _, sid := range scoped {
			args = append(args, sid)
		}
		query += ` OR created_at < ?)`
		args = append(args, nanos(staleBefore)) // zero time = 0 = matches nothing
	}
	query += ` ORDER BY id`

	rows, err := tx.QueryContext(ctx, query, args...)
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
	if err := rows.Err(); err != nil {
		_ = rows.Close()
		return nil, fmt.Errorf("claim outbox: rows: %w", err)
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

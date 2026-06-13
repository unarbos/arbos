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
// undelivered messages to the user, claimed atomically by the door that holds
// the originating conversation open (broadcast-class rows, which have no
// conversation, by any door). Delivery semantics live in internal/outbox;
// this is dumb rows, like the atom and plan files.

// Notify appends one undelivered message for the user, stamped by the session
// that spoke. Sugar for NotifyFrom with no source (never coalesced).
func (s *Store) Notify(ctx context.Context, text, session string) error {
	return s.NotifyFrom(ctx, text, session, "")
}

// ownerChainMax bounds the spawn-chain walk; deeper nesting than this is a
// cycle or a bug, and the message simply stays addressed where the walk stops.
const ownerChainMax = 8

// NotifyFrom appends one undelivered message, BOUND to the conversation it
// came from:
//
//   - the stamping session's owner chain (chat → wake → delegate …) resolves
//     to its root conversation, so machine-spawned work speaks into the chat
//     that created it, however deep the nesting;
//   - the principal rides along as the durable address (so a future
//     multi-user gateway never crosses principals); delivery is scoped to
//     that conversation's own door — there is no fallback to another chat.
//
// A non-empty source (e.g. "node:12") makes the message COALESCING: a new
// firing supersedes its own undelivered predecessor, so a recurring ping
// never piles up while its conversation is closed — the next time that door
// opens it gets the latest reading, not the backlog. The same latest-per-
// source rule the projection applies to injected context (core.ContextPayload).
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

// ClaimOutboxFor is the addressed claim: it takes the principal's messages —
// never another principal's — and only those that belong to a session this
// door holds open:
//
//   - messages whose session is in sessions (that conversation has this door
//     open) deliver now;
//   - broadcast-class messages (no session, or the kernel's) deliver to any
//     door, since they have no conversation of their own to wait for.
//
// There is no cross-session fallback: a notice belongs to the conversation
// that created it and waits, durably, for that conversation to reopen. It is
// never rerouted into an unrelated chat — so a background automation's voice
// (e.g. a throwaway `arbos -once` session that never reopens) stays self-
// contained instead of spilling into whatever door happens to be open.
func (s *Store) ClaimOutboxFor(ctx context.Context, via, principal string, sessions []string) ([]outbox.Message, error) {
	s.writeMu.Lock()
	defer s.writeMu.Unlock()

	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return nil, fmt.Errorf("claim outbox: begin: %w", err)
	}
	defer func() { _ = tx.Rollback() }()

	// Address first: only this principal's mail (rows from before the
	// principal column exist as '' and stay claimable by any door). Then
	// scope to the conversations this door holds open, plus the broadcast
	// markers that belong to no conversation. scoped is never empty (the
	// broadcast markers are always present), so the IN clause is well-formed
	// even when this door holds no sessions.
	scoped := append(append([]string{}, outbox.BroadcastSessions...), sessions...)
	if len(scoped) == 0 {
		// Defensive: the broadcast markers keep scoped non-empty in practice,
		// so the IN-clause placeholder math below never underflows. Guard
		// anyway — a future edit emptying BroadcastSessions must degrade to
		// "claim nothing", not panic under the write lock.
		return nil, nil
	}
	query := `SELECT id, message, session_id, created_at FROM outbox
		  WHERE delivered_at = 0 AND principal IN (?, '')
		  AND session_id IN (?` + strings.Repeat(",?", len(scoped)-1) + `)
		  ORDER BY id`
	args := []any{principal}
	for _, sid := range scoped {
		args = append(args, sid)
	}

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

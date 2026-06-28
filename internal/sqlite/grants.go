package sqlite

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"time"

	"github.com/unarbos/arbos/internal/share"
)

// ErrNoGrant is returned by Grant when a token resolves to no row — the
// uniform "not a valid link" answer (expired, revoked, or never existed all
// look the same to an unauthenticated holder).
var ErrNoGrant = errors.New("no such grant")

// PutGrant inserts (or replaces) a share grant keyed by its token. Re-putting
// the same token is idempotent.
func (s *Store) PutGrant(ctx context.Context, g share.Grant) error {
	s.writeMu.Lock()
	defer s.writeMu.Unlock()
	var expires int64
	if !g.Expires.IsZero() {
		expires = g.Expires.UnixNano()
	}
	_, err := s.db.ExecContext(ctx,
		`INSERT OR REPLACE INTO grants
		   (token, scope_kind, scope_ref, perm, expires_at, created_at)
		 VALUES (?, ?, ?, ?, ?, ?)`,
		g.Token, string(g.Scope.Kind), g.Scope.Ref, int(g.Perm),
		expires, g.Created.UnixNano(),
	)
	if err != nil {
		return fmt.Errorf("put grant: %w", err)
	}
	return nil
}

// Grant resolves a token to its grant, or ErrNoGrant when absent.
func (s *Store) Grant(ctx context.Context, token string) (share.Grant, error) {
	row := s.db.QueryRowContext(ctx,
		`SELECT token, scope_kind, scope_ref, perm, expires_at, created_at
		 FROM grants WHERE token = ?`, token)
	var (
		g                  share.Grant
		kind               string
		perm               int
		expiresAt, created int64
	)
	err := row.Scan(&g.Token, &kind, &g.Scope.Ref, &perm, &expiresAt, &created)
	if errors.Is(err, sql.ErrNoRows) {
		return share.Grant{}, ErrNoGrant
	}
	if err != nil {
		return share.Grant{}, fmt.Errorf("grant %q: %w", token, err)
	}
	g.Scope.Kind = share.ScopeKind(kind)
	g.Perm = share.Perm(perm)
	if expiresAt != 0 {
		g.Expires = time.Unix(0, expiresAt)
	}
	g.Created = time.Unix(0, created)
	return g, nil
}

// RevokeGrant deletes a grant by token. Revoking an absent token is a no-op
// success.
func (s *Store) RevokeGrant(ctx context.Context, token string) error {
	s.writeMu.Lock()
	defer s.writeMu.Unlock()
	if _, err := s.db.ExecContext(ctx, `DELETE FROM grants WHERE token = ?`, token); err != nil {
		return fmt.Errorf("revoke grant: %w", err)
	}
	return nil
}

// ShareSeat is one Matrix room membership a share token created — the session
// whose room a guest joined and the trusted display name they joined under.
type ShareSeat struct {
	Session string
	Author  string
}

// AddShareSeat durably records that a share token seated a guest in a session's
// room, so a later revoke can kick exactly that guest even across a restart.
// Idempotent: re-seating the same (token, session, author) is a no-op.
func (s *Store) AddShareSeat(ctx context.Context, token, session, author string) error {
	s.writeMu.Lock()
	defer s.writeMu.Unlock()
	_, err := s.db.ExecContext(ctx,
		`INSERT OR IGNORE INTO share_seats (token, session, author) VALUES (?, ?, ?)`,
		token, session, author)
	if err != nil {
		return fmt.Errorf("add share seat: %w", err)
	}
	return nil
}

// ShareSeats returns the seats a token created — the guests to kick when it is
// revoked.
func (s *Store) ShareSeats(ctx context.Context, token string) ([]ShareSeat, error) {
	rows, err := s.db.QueryContext(ctx,
		`SELECT session, author FROM share_seats WHERE token = ?`, token)
	if err != nil {
		return nil, fmt.Errorf("share seats %q: %w", token, err)
	}
	defer func() { _ = rows.Close() }()
	var out []ShareSeat
	for rows.Next() {
		var seat ShareSeat
		if err := rows.Scan(&seat.Session, &seat.Author); err != nil {
			return nil, fmt.Errorf("scan share seat: %w", err)
		}
		out = append(out, seat)
	}
	return out, rows.Err()
}

// DeleteShareSeats drops every seat a token created — called as the token is
// revoked, after its guests have been kicked.
func (s *Store) DeleteShareSeats(ctx context.Context, token string) error {
	s.writeMu.Lock()
	defer s.writeMu.Unlock()
	if _, err := s.db.ExecContext(ctx, `DELETE FROM share_seats WHERE token = ?`, token); err != nil {
		return fmt.Errorf("delete share seats: %w", err)
	}
	return nil
}

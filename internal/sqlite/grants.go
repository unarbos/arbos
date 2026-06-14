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

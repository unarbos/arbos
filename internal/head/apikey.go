package head

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"errors"
	"time"
)

// keyPrefix tags every minted key so a leaked secret is greppable and a
// dashboard can show "sk-arbos-…" without revealing the rest.
const keyPrefix = "sk-arbos-"

// APIKey is the stored, non-secret view of a key: enough to list and manage it
// in a dashboard, never the secret itself (only the sha256 hash is kept).
type APIKey struct {
	ID        string `json:"id"`
	AccountID string `json:"account_id"`
	Name      string `json:"name,omitempty"`
	Prefix    string `json:"prefix"`
	Created   int64  `json:"created"`
	LastUsed  int64  `json:"last_used,omitempty"`
	Revoked   bool   `json:"revoked"`
}

// MintKey issues a new API key for an account and returns the full secret
// exactly once — only its hash is persisted, so it can never be recovered.
func (s *Store) MintKey(ctx context.Context, accountID, name string) (APIKey, string, error) {
	var raw [24]byte
	if _, err := rand.Read(raw[:]); err != nil {
		return APIKey{}, "", err
	}
	secret := keyPrefix + hex.EncodeToString(raw[:])
	sum := sha256.Sum256([]byte(secret))
	rec := APIKey{
		ID:        newID("key"),
		AccountID: accountID,
		Name:      name,
		Prefix:    secret[:len(keyPrefix)+6],
		Created:   time.Now().Unix(),
	}
	_, err := s.db.ExecContext(ctx,
		`INSERT INTO api_keys (id, account_id, name, hash, prefix, created)
		 VALUES (?, ?, ?, ?, ?, ?)`,
		rec.ID, rec.AccountID, rec.Name, hex.EncodeToString(sum[:]), rec.Prefix, rec.Created)
	if err != nil {
		return APIKey{}, "", err
	}
	return rec, secret, nil
}

// ResolveKey looks up a live key by its presented secret and stamps last_used.
// A revoked or unknown key returns ErrNotFound — the gateway maps that to 401
// without leaking which case it was.
func (s *Store) ResolveKey(ctx context.Context, secret string) (APIKey, error) {
	sum := sha256.Sum256([]byte(secret))
	var k APIKey
	err := s.db.QueryRowContext(ctx,
		`SELECT id, account_id, name, prefix, created, last_used, revoked
		 FROM api_keys WHERE hash = ? AND revoked = 0`,
		hex.EncodeToString(sum[:])).
		Scan(&k.ID, &k.AccountID, &k.Name, &k.Prefix, &k.Created, &k.LastUsed, &k.Revoked)
	if errors.Is(err, sql.ErrNoRows) {
		return APIKey{}, ErrNotFound
	}
	if err != nil {
		return APIKey{}, err
	}
	// Coarse last_used stamp: only write when the recorded value is stale, so
	// the hot auth path (every chat completion) is a read, not a write, except
	// at most once a minute per key. Best-effort — a failure must not fail the
	// request.
	if now := time.Now().Unix(); now-k.LastUsed > 60 {
		_, _ = s.db.ExecContext(ctx, `UPDATE api_keys SET last_used = ? WHERE id = ?`, now, k.ID)
	}
	return k, nil
}

// ListKeys returns an account's keys (newest first), for a dashboard.
func (s *Store) ListKeys(ctx context.Context, accountID string) ([]APIKey, error) {
	rows, err := s.db.QueryContext(ctx,
		`SELECT id, account_id, name, prefix, created, last_used, revoked
		 FROM api_keys WHERE account_id = ? ORDER BY created DESC`, accountID)
	if err != nil {
		return nil, err
	}
	defer func() { _ = rows.Close() }()
	var out []APIKey
	for rows.Next() {
		var k APIKey
		if err := rows.Scan(&k.ID, &k.AccountID, &k.Name, &k.Prefix, &k.Created, &k.LastUsed, &k.Revoked); err != nil {
			return nil, err
		}
		out = append(out, k)
	}
	return out, rows.Err()
}

// RevokeKey marks a key dead. Idempotent: revoking an already-dead or unknown
// key is not an error.
func (s *Store) RevokeKey(ctx context.Context, accountID, keyID string) error {
	_, err := s.db.ExecContext(ctx,
		`UPDATE api_keys SET revoked = 1 WHERE id = ? AND account_id = ?`, keyID, accountID)
	return err
}

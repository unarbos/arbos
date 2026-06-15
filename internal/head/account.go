package head

import (
	"context"
	"database/sql"
	"errors"
	"strings"
	"time"
)

// Account is a non-KYC, agent-first account: an opaque id and a tier, with all
// value tracked on the ledger. No email or login is required to exist — an
// agent can sign up, get a key, and be funded by crypto without ever touching
// a human identity provider.
type Account struct {
	ID      string `json:"id"`
	Tier    string `json:"tier"`
	Email   string `json:"email,omitempty"`
	Created int64  `json:"created"`
}

// ErrNotFound is returned when an account, key, or row does not exist.
var ErrNotFound = errors.New("head: not found")

// CreateAccount mints a fresh anonymous account and seeds its balance row at
// zero in the same transaction, so a balance read never finds a missing row.
func (s *Store) CreateAccount(ctx context.Context) (Account, error) {
	acct := Account{ID: newID("acct"), Tier: "agent", Created: time.Now().Unix()}
	s.writeMu.Lock()
	defer s.writeMu.Unlock()
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return Account{}, err
	}
	defer func() { _ = tx.Rollback() }()
	if _, err := tx.ExecContext(ctx,
		`INSERT INTO accounts (id, tier, created) VALUES (?, ?, ?)`,
		acct.ID, acct.Tier, acct.Created); err != nil {
		return Account{}, err
	}
	if _, err := tx.ExecContext(ctx,
		`INSERT INTO account_balances (account_id, balance_micro) VALUES (?, 0)`,
		acct.ID); err != nil {
		return Account{}, err
	}
	if err := tx.Commit(); err != nil {
		return Account{}, err
	}
	return acct, nil
}

// DepositAddress returns the account's EVM deposit address, generating and
// persisting one on first use. Stable per account, so a payer fetches it once
// and reuses it; a deposit is attributed by which account owns the address.
func (s *Store) DepositAddress(ctx context.Context, accountID string) (string, error) {
	var addr string
	err := s.db.QueryRowContext(ctx, `SELECT address FROM deposit_addresses WHERE account_id = ?`, accountID).Scan(&addr)
	if err == nil {
		return addr, nil
	}
	if !errors.Is(err, sql.ErrNoRows) {
		return "", err
	}
	addr, priv, err := newDepositKey()
	if err != nil {
		return "", err
	}
	s.writeMu.Lock()
	defer s.writeMu.Unlock()
	// A concurrent caller may have inserted between the read and here; ignore
	// the conflict and re-read the winner so the address stays stable.
	if _, err := s.db.ExecContext(ctx,
		`INSERT INTO deposit_addresses (account_id, address, privkey, created) VALUES (?, ?, ?, ?)
		 ON CONFLICT(account_id) DO NOTHING`,
		accountID, addr, priv, time.Now().Unix()); err != nil {
		return "", err
	}
	if err := s.db.QueryRowContext(ctx, `SELECT address FROM deposit_addresses WHERE account_id = ?`, accountID).Scan(&addr); err != nil {
		return "", err
	}
	return addr, nil
}

// AccountByDepositAddress resolves which account owns a deposit address
// (case-insensitive), or ErrNotFound.
func (s *Store) AccountByDepositAddress(ctx context.Context, address string) (string, error) {
	var acct string
	err := s.db.QueryRowContext(ctx,
		`SELECT account_id FROM deposit_addresses WHERE address = ?`, strings.ToLower(address)).Scan(&acct)
	if errors.Is(err, sql.ErrNoRows) {
		return "", ErrNotFound
	}
	return acct, err
}

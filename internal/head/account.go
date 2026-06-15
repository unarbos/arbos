package head

import (
	"context"
	"errors"
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

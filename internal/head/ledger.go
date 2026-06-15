package head

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"time"
)

// Entry kinds and reasons. Kept as plain strings so a new funding rail or
// debit source needs no schema change — just a new reason.
const (
	kindCredit = "credit"
	kindDebit  = "debit"

	reasonFunding    = "funding"    // money in: crypto deposit, Stripe, manual
	reasonInference  = "inference"  // money out: a metered LLM call
	reasonPromo      = "promo"      // signup allowance
	reasonAdjustment = "adjustment" // manual correction
)

// Post writes one ledger entry and moves the materialized balance in the same
// transaction. ref is the idempotency key: a repeated ref is a silent no-op
// (no second entry, no double-move), which is what lets a redelivered deposit
// confirmation or a retried usage settlement be replayed safely. A negative or
// zero amount is rejected — kind, not sign, decides direction.
func (s *Store) Post(ctx context.Context, accountID, kind string, amountMicro int64, reason, ref string, meta any) error {
	if amountMicro <= 0 {
		return errors.New("head: ledger amount must be positive")
	}
	if kind != kindCredit && kind != kindDebit {
		return errors.New("head: ledger kind must be credit or debit")
	}
	metaJSON := []byte("{}")
	if meta != nil {
		if b, err := json.Marshal(meta); err == nil {
			metaJSON = b
		}
	}
	s.writeMu.Lock()
	defer s.writeMu.Unlock()
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback() }()

	res, err := tx.ExecContext(ctx,
		`INSERT INTO ledger_entries (id, account_id, ts, kind, amount_micro, reason, ref, meta)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?)
		 ON CONFLICT(ref) DO NOTHING`,
		newID("led"), accountID, time.Now().Unix(), kind, amountMicro, reason, ref, string(metaJSON))
	if err != nil {
		return err
	}
	if n, _ := res.RowsAffected(); n == 0 {
		// Duplicate ref: already posted. Commit the (empty) tx and return ok.
		return tx.Commit()
	}

	delta := amountMicro
	if kind == kindDebit {
		delta = -amountMicro
	}
	if _, err := tx.ExecContext(ctx,
		`UPDATE account_balances SET balance_micro = balance_micro + ? WHERE account_id = ?`,
		delta, accountID); err != nil {
		return err
	}
	return tx.Commit()
}

// Credit and Debit are the two callers' verbs over Post.
func (s *Store) Credit(ctx context.Context, accountID string, amountMicro int64, reason, ref string, meta any) error {
	return s.Post(ctx, accountID, kindCredit, amountMicro, reason, ref, meta)
}

func (s *Store) Debit(ctx context.Context, accountID string, amountMicro int64, reason, ref string, meta any) error {
	return s.Post(ctx, accountID, kindDebit, amountMicro, reason, ref, meta)
}

// Balance returns the account's current balance in micro-USD. A missing row
// (which CreateAccount prevents) reads as zero.
func (s *Store) Balance(ctx context.Context, accountID string) (int64, error) {
	var bal int64
	err := s.db.QueryRowContext(ctx,
		`SELECT balance_micro FROM account_balances WHERE account_id = ?`, accountID).Scan(&bal)
	if errors.Is(err, sql.ErrNoRows) {
		return 0, nil
	}
	return bal, err
}

// RecordUsage logs one metered inference call for attribution and dashboards.
// It is separate from the debit (the ledger owns money; this owns the detail)
// and best-effort: a logging failure must not undo a settled debit.
func (s *Store) RecordUsage(ctx context.Context, accountID, keyID, model, upstream string, promptTokens, completionTokens int, costMicro int64, ledgerRef string) {
	_, _ = s.db.ExecContext(ctx,
		`INSERT INTO usage_records
		   (id, account_id, key_id, ts, model, upstream, prompt_tokens, completion_tokens, cost_micro, ledger_entry_id)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
		newID("use"), accountID, keyID, time.Now().Unix(), model, upstream,
		promptTokens, completionTokens, costMicro, ledgerRef)
}

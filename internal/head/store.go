// Package head is the arbos.life account backend: non-KYC, agent-first
// accounts with a prepaid balance, scoped API keys, and an OpenAI-compatible
// inference gateway that meters and forwards to OpenRouter (Chutes and other
// upstreams later). It is a sibling to the forest relay (internal/forest),
// not a replacement: the relay still owns device leases and tunnels; the head
// owns money, keys, and inference. Both can run in one process later, but this
// package stands alone so the relay is never disturbed while it is built.
//
// The ledger is the spine. Funding (MetaMask now, TAO/Stripe later) and
// inference debits are entries against one append-only, double-entry ledger
// keyed by account. Money is int64 micro-USD throughout — never a float — so
// rounding is explicit and totals are exact.
package head

import (
	"context"
	"crypto/rand"
	"database/sql"
	"encoding/hex"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"sync"

	_ "modernc.org/sqlite" // registers the "sqlite" database/sql driver
)

// microPerUSD is the fixed-point scale: balances and prices are integer
// millionths of a US dollar, so $1.00 == 1_000_000 and a tenth of a cent is
// representable without float drift.
const microPerUSD = 1_000_000

// MicroFromUSD converts a USD amount to integer micro-USD, rounding to the
// nearest micro so a fractional input (a credit amount, a promo) never silently
// truncates. The one place float dollars cross into the integer ledger.
func MicroFromUSD(usd float64) int64 { return int64(math.Round(usd * microPerUSD)) }

// Store is the head's durable state on a single SQLite database. Every query
// method hangs off it; HTTP wiring lives on Head. The queries are plain SQL, so
// a Postgres-backed Store is a future drop-in — though Head holds the concrete
// type today, so that swap means changing this type, not satisfying an
// interface. Keeping the driver import here means nothing else depends on it.
type Store struct {
	db *sql.DB
	// writeMu serializes multi-statement write transactions within this
	// process. SQLite allows one writer at a time; a read-then-write
	// transaction (the ledger's insert-then-update, account creation's two
	// inserts) can otherwise deadlock against a concurrent one and fail past
	// busy_timeout. Serializing the few transactional writers keeps money
	// movements exact; reads and single-statement writes stay concurrent under
	// WAL. This mirrors internal/sqlite's proven discipline.
	writeMu sync.Mutex
}

// Open opens (creating if absent) the head database at path and applies the
// schema. WAL keeps reads concurrent with the single writer; foreign keys are
// enforced so a stray account_id can't orphan a key or a ledger row.
func Open(path string) (*Store, error) {
	// Create the parent dir so the documented default (~/.config/arbos-head/…)
	// works on a fresh self-host; modernc sqlite won't make missing dirs.
	if dir := filepath.Dir(path); dir != "" && dir != "." {
		_ = os.MkdirAll(dir, 0o700)
	}
	dsn := fmt.Sprintf("file:%s?_pragma=busy_timeout(5000)&_pragma=journal_mode(WAL)&_pragma=foreign_keys(1)", path)
	db, err := sql.Open("sqlite", dsn)
	if err != nil {
		return nil, fmt.Errorf("head: open sqlite: %w", err)
	}
	s := &Store{db: db}
	if err := s.migrate(context.Background()); err != nil {
		_ = db.Close()
		return nil, err
	}
	return s, nil
}

// Close releases the database handle.
func (s *Store) Close() error { return s.db.Close() }

// migrate applies the forward-only schema. Every statement is idempotent;
// later schema changes append new statements and never edit these (ADR-0005).
func (s *Store) migrate(ctx context.Context) error {
	const schema = `
CREATE TABLE IF NOT EXISTS accounts (
    id      TEXT PRIMARY KEY,
    tier    TEXT    NOT NULL DEFAULT 'agent',
    email   TEXT    NOT NULL DEFAULT '',
    created INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS api_keys (
    id         TEXT PRIMARY KEY,
    account_id TEXT    NOT NULL REFERENCES accounts(id),
    name       TEXT    NOT NULL DEFAULT '',
    hash       TEXT    NOT NULL UNIQUE,        -- sha256 hex of the full key
    prefix     TEXT    NOT NULL,               -- shown in dashboards; the rest is secret
    created    INTEGER NOT NULL,
    last_used  INTEGER NOT NULL DEFAULT 0,
    revoked    INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_api_keys_account ON api_keys(account_id);

-- Append-only money. amount_micro is always positive; kind carries the sign.
-- ref is a caller-supplied idempotency key (a tx hash, a usage id): a repeated
-- ref is a no-op, which makes redelivered chain confirmations and retried
-- settlements safe.
CREATE TABLE IF NOT EXISTS ledger_entries (
    id           TEXT PRIMARY KEY,
    account_id   TEXT    NOT NULL REFERENCES accounts(id),
    ts           INTEGER NOT NULL,
    kind         TEXT    NOT NULL,             -- credit | debit
    amount_micro INTEGER NOT NULL,
    reason       TEXT    NOT NULL,             -- funding | inference | promo | adjustment
    ref          TEXT    NOT NULL UNIQUE,
    meta         TEXT    NOT NULL DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS idx_ledger_account ON ledger_entries(account_id, ts);

-- Materialized balance, updated in the same transaction as each ledger entry
-- so a read never has to fold the whole log.
CREATE TABLE IF NOT EXISTS account_balances (
    account_id    TEXT PRIMARY KEY REFERENCES accounts(id),
    balance_micro INTEGER NOT NULL DEFAULT 0
);

-- Per-account EVM deposit address: a deposit is attributed by which account
-- owns the destination address, so no memo/tag is needed. privkey is the
-- 32-byte secp256k1 key that later sweeps funds to a cold treasury.
CREATE TABLE IF NOT EXISTS deposit_addresses (
    account_id TEXT PRIMARY KEY REFERENCES accounts(id),
    address    TEXT    NOT NULL UNIQUE,  -- lowercased 0x EVM address
    privkey    BLOB    NOT NULL,
    created    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS usage_records (
    id                TEXT PRIMARY KEY,
    account_id        TEXT    NOT NULL,
    key_id            TEXT    NOT NULL DEFAULT '',
    ts                INTEGER NOT NULL,
    model             TEXT    NOT NULL DEFAULT '',
    upstream          TEXT    NOT NULL DEFAULT 'openrouter',
    prompt_tokens     INTEGER NOT NULL DEFAULT 0,
    completion_tokens INTEGER NOT NULL DEFAULT 0,
    cost_micro        INTEGER NOT NULL DEFAULT 0,
    ledger_entry_id   TEXT    NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_usage_account ON usage_records(account_id, ts);
`
	if _, err := s.db.ExecContext(ctx, schema); err != nil {
		return fmt.Errorf("head: migrate: %w", err)
	}
	return nil
}

// newID returns a prefixed random identifier, e.g. newID("acct") ->
// "acct_9f2c…". 16 random bytes is plenty of collision margin.
func newID(prefix string) string {
	var b [16]byte
	_, _ = rand.Read(b[:])
	return prefix + "_" + hex.EncodeToString(b[:])
}

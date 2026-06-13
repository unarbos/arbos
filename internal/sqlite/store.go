// Package sqlite is the durable ports.SessionStore: an append-only event log on
// modernc.org/sqlite (pure Go, no cgo) so the kernel stays a single static
// binary. It is a drop-in for the in-memory fake — it passes the exact same
// porttest.RunSessionStoreContract and the engine's store-parameterized golden
// tests (ADR-0005).
//
// Persisted shape mirrors the data model: one row per core.Event with the
// payload stored as JSON via the core codec (so ProviderMeta and every payload
// field round-trips, ADR-0003/0008), the kind as an indexable discriminator
// column, and the schema Version per row so an older binary can upcast on read
// (ADR-0010). An FTS5 virtual table over message/context text backs session
// search.
package sqlite

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"strings"
	"sync"
	"time"

	_ "modernc.org/sqlite" // registers the "sqlite" database/sql driver

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/ports"
)

// Store implements ports.SessionStore over a single SQLite database.
type Store struct {
	db *sql.DB
	// writeMu serializes writers within this process. SQLite itself allows one
	// writer at a time; the engine guarantees a single writer per session, but
	// distinct session actors append concurrently. Computing the next per-session
	// Seq is a read-then-insert that must be atomic, so we serialize the whole
	// write path here (reads stay concurrent under WAL). This keeps Seq strictly
	// monotonic without leaning on SQLite busy-retry semantics.
	writeMu sync.Mutex
}

var _ ports.SessionStore = (*Store)(nil)

// Open opens (creating if absent) a SQLite-backed store at path and applies the
// schema. Use a real file path; an in-memory DSN would give each connection its
// own database. Call Close when done.
func Open(path string) (*Store, error) {
	dsn := fmt.Sprintf("file:%s?_pragma=busy_timeout(5000)&_pragma=journal_mode(WAL)&_pragma=foreign_keys(1)", path)
	db, err := sql.Open("sqlite", dsn)
	if err != nil {
		return nil, fmt.Errorf("open sqlite: %w", err)
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

// migrate applies the forward-only schema. Each statement is idempotent
// (IF NOT EXISTS); future schema changes append new migration steps, never edit
// existing ones (ADR-0005). Databases created before Session.TokenCount was
// removed may carry a vestigial token_count column (NOT NULL DEFAULT 0); no
// statement references it, so it is left in place rather than dropped.
func (s *Store) migrate(ctx context.Context) error {
	const schema = `
CREATE TABLE IF NOT EXISTS sessions (
    id          TEXT PRIMARY KEY,
    parent_id   TEXT    NOT NULL DEFAULT '',
    status      TEXT    NOT NULL,
    model       TEXT    NOT NULL DEFAULT '',
    principal   TEXT    NOT NULL DEFAULT '',
    origin      TEXT    NOT NULL DEFAULT '',
    owner       TEXT    NOT NULL DEFAULT '', -- chat this session serves ('' = a chat itself)
    spawned_by  TEXT    NOT NULL DEFAULT '', -- what spawned it, e.g. 'node:12'
    web_search  INTEGER NOT NULL DEFAULT 0, -- durable per-session web-search toggle
    web_fetch   INTEGER NOT NULL DEFAULT 0, -- durable per-session web-fetch toggle
    image_gen   INTEGER NOT NULL DEFAULT 0, -- durable per-session image-generation toggle
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS events (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT    NOT NULL REFERENCES sessions(id),
    seq        INTEGER NOT NULL,
    turn_id    INTEGER NOT NULL DEFAULT 0,
    kind       TEXT    NOT NULL,
    version    INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    payload    BLOB    NOT NULL,
    UNIQUE(session_id, seq)
);

CREATE INDEX IF NOT EXISTS idx_events_session_seq ON events(session_id, seq);

CREATE VIRTUAL TABLE IF NOT EXISTS events_fts USING fts5(
    content,
    session_id UNINDEXED,
    event_id   UNINDEXED
);

-- atoms: the agent's durable comprehensions. One global set (no scope column),
-- so every session reads and writes the same memory — one agent learning across
-- all conversations. Upsert by id (PRIMARY KEY) is how the curator merges.
CREATE TABLE IF NOT EXISTS atoms (
    id         TEXT PRIMARY KEY,
    content    TEXT    NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE VIRTUAL TABLE IF NOT EXISTS atoms_fts USING fts5(
    content,
    atom_id UNINDEXED
);

-- mind_checkpoints: per-session high-water mark of the last event seq the
-- curator has folded into atoms, so curation is incremental (never re-reads the
-- whole log).
CREATE TABLE IF NOT EXISTS mind_checkpoints (
    session_id TEXT PRIMARY KEY,
    last_seq   INTEGER NOT NULL
);

-- plan_nodes: the agent's durable intent — a forest of goals (see
-- internal/plan). Nodes are goals, not tasks: executing one is a row in
-- plan_attempts. Integer ids on purpose: the model references nodes tersely
-- ("#7"). Times are UnixNano with 0 = unset; every_ns is a recurrence period
-- in nanoseconds (0 = one-shot).
CREATE TABLE IF NOT EXISTS plan_nodes (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    plan_id    INTEGER NOT NULL,            -- root's id; a root points at itself
    parent_id  INTEGER NOT NULL DEFAULT 0,  -- 0 = root
    seq        INTEGER NOT NULL,            -- sibling order = implicit dependency
    kind       TEXT    NOT NULL,            -- achieve | maintain
    goal       TEXT    NOT NULL,
    check_expr TEXT    NOT NULL DEFAULT '', -- how to verify done ('' = self-report)
    cmd        TEXT    NOT NULL DEFAULT '', -- shell executor: kernel-run command ('' = not a shell node)
    cond       TEXT    NOT NULL DEFAULT '', -- gate: shell predicate; do fires only on exit 0 ('' = no gate)
    notify     TEXT    NOT NULL DEFAULT '', -- notify executor: message emitted to the outbox, no model turn
    wake       INTEGER NOT NULL DEFAULT 0,  -- one-shot callback: summon a model turn when ready
    status     TEXT    NOT NULL,            -- pending|active|blocked|done|cancelled|failed
    outcome    TEXT    NOT NULL DEFAULT '',
    assignee   TEXT    NOT NULL DEFAULT 'agent',
    owner      TEXT    NOT NULL DEFAULT '', -- claiming session
    origin     TEXT    NOT NULL DEFAULT '', -- creating session: where its voice routes
    after_at   INTEGER NOT NULL DEFAULT 0,  -- not ready before
    every_ns   INTEGER NOT NULL DEFAULT 0,  -- recurrence period
    next_due   INTEGER NOT NULL DEFAULT 0,  -- next firing for recurring nodes
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_plan_nodes_tree ON plan_nodes(plan_id, parent_id, seq);

-- plan_attempts: append-only execution history of plan nodes. Never updated
-- or deleted — failed attempts are knowledge. workspace and verified_by are
-- day-one columns for attempt-isolated worktrees and independent verifiers.
CREATE TABLE IF NOT EXISTS plan_attempts (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    node_id     INTEGER NOT NULL REFERENCES plan_nodes(id),
    session_id  TEXT    NOT NULL,
    verdict     TEXT    NOT NULL,            -- success | fail | inconclusive
    outcome     TEXT    NOT NULL DEFAULT '',
    verified_by TEXT    NOT NULL DEFAULT 'self',
    workspace   TEXT    NOT NULL DEFAULT '',
    created_at  INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_plan_attempts_node ON plan_attempts(node_id);

-- outbox: the agent's undelivered messages to the user (see internal/outbox).
-- Delivery is claim-then-deliver: a door sets delivered_at/delivered_via
-- atomically before showing the message, so racing doors never double-deliver.
CREATE TABLE IF NOT EXISTS outbox (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    message       TEXT    NOT NULL,
    session_id    TEXT    NOT NULL DEFAULT '',
    created_at    INTEGER NOT NULL,
    delivered_at  INTEGER NOT NULL DEFAULT 0,
    delivered_via TEXT    NOT NULL DEFAULT ''
);

CREATE INDEX IF NOT EXISTS idx_outbox_undelivered ON outbox(delivered_at) WHERE delivered_at = 0;
`
	if _, err := s.db.ExecContext(ctx, schema); err != nil {
		return fmt.Errorf("migrate: %w", err)
	}
	// Forward-only column additions (ADR-0005): CREATE TABLE IF NOT EXISTS
	// does not retrofit columns onto pre-existing databases, so each later
	// column also appends an ALTER, skipped when the column already exists
	// (fresh databases get it from the CREATE above). Existence is probed via
	// table_info rather than by matching the driver's "duplicate column"
	// error text, which is wording we don't control.
	for _, t := range []struct {
		table  string
		alters []struct{ col, stmt string }
	}{
		{"plan_nodes", []struct{ col, stmt string }{
			{"cmd", `ALTER TABLE plan_nodes ADD COLUMN cmd TEXT NOT NULL DEFAULT ''`},
			{"wake", `ALTER TABLE plan_nodes ADD COLUMN wake INTEGER NOT NULL DEFAULT 0`},
			{"notify", `ALTER TABLE plan_nodes ADD COLUMN notify TEXT NOT NULL DEFAULT ''`},
			{"origin", `ALTER TABLE plan_nodes ADD COLUMN origin TEXT NOT NULL DEFAULT ''`},
			{"cond", `ALTER TABLE plan_nodes ADD COLUMN cond TEXT NOT NULL DEFAULT ''`},
		}},
		{"sessions", []struct{ col, stmt string }{
			{"owner", `ALTER TABLE sessions ADD COLUMN owner TEXT NOT NULL DEFAULT ''`},
			{"spawned_by", `ALTER TABLE sessions ADD COLUMN spawned_by TEXT NOT NULL DEFAULT ''`},
			{"web_search", `ALTER TABLE sessions ADD COLUMN web_search INTEGER NOT NULL DEFAULT 0`},
			{"web_fetch", `ALTER TABLE sessions ADD COLUMN web_fetch INTEGER NOT NULL DEFAULT 0`},
			{"image_gen", `ALTER TABLE sessions ADD COLUMN image_gen INTEGER NOT NULL DEFAULT 0`},
		}},
		{"outbox", []struct{ col, stmt string }{
			{"principal", `ALTER TABLE outbox ADD COLUMN principal TEXT NOT NULL DEFAULT ''`},
			{"source", `ALTER TABLE outbox ADD COLUMN source TEXT NOT NULL DEFAULT ''`},
		}},
	} {
		cols, err := s.tableColumns(ctx, t.table)
		if err != nil {
			return err
		}
		for _, a := range t.alters {
			if cols[a.col] {
				continue
			}
			if _, err := s.db.ExecContext(ctx, a.stmt); err != nil {
				return fmt.Errorf("migrate: %s: %w", a.stmt, err)
			}
		}
	}
	return nil
}

// tableColumns probes a table's existing columns for the forward-only ALTER
// pass (ADR-0005): existence via table_info, not by matching driver error text.
func (s *Store) tableColumns(ctx context.Context, table string) (map[string]bool, error) {
	cols := make(map[string]bool)
	rows, err := s.db.QueryContext(ctx, `SELECT name FROM pragma_table_info(?)`, table)
	if err != nil {
		return nil, fmt.Errorf("migrate: table_info %s: %w", table, err)
	}
	defer func() { _ = rows.Close() }()
	for rows.Next() {
		var name string
		if err := rows.Scan(&name); err != nil {
			return nil, fmt.Errorf("migrate: table_info %s: %w", table, err)
		}
		cols[name] = true
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("migrate: table_info %s: %w", table, err)
	}
	return cols, nil
}

func (s *Store) CreateSession(ctx context.Context, sess core.Session) error {
	s.writeMu.Lock()
	defer s.writeMu.Unlock()
	_, err := s.db.ExecContext(ctx,
		`INSERT INTO sessions (id, parent_id, status, model, principal, origin, owner, spawned_by, web_search, web_fetch, image_gen, created_at, updated_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
		string(sess.ID), string(sess.ParentID), string(sess.Status), sess.Model,
		sess.Principal, sess.Origin, string(sess.Owner), sess.SpawnedBy, boolToInt(sess.WebSearch), boolToInt(sess.WebFetch), boolToInt(sess.ImageGen),
		sess.CreatedAt.UnixNano(), sess.UpdatedAt.UnixNano(),
	)
	if err != nil {
		return fmt.Errorf("create session %q: %w", sess.ID, err)
	}
	return nil
}

func (s *Store) Get(ctx context.Context, id core.SessionID) (core.Session, error) {
	row := s.db.QueryRowContext(ctx,
		`SELECT id, parent_id, status, model, principal, origin, owner, spawned_by, web_search, web_fetch, image_gen, created_at, updated_at
		 FROM sessions WHERE id = ?`, string(id))
	var (
		sess                          core.Session
		idStr, parentID               string
		status, owner                 string
		webSearch, webFetch, imageGen int
		createdAt, updated            int64
	)
	err := row.Scan(&idStr, &parentID, &status, &sess.Model,
		&sess.Principal, &sess.Origin, &owner, &sess.SpawnedBy, &webSearch, &webFetch, &imageGen, &createdAt, &updated)
	if errors.Is(err, sql.ErrNoRows) {
		return core.Session{}, ports.ErrSessionNotFound
	}
	if err != nil {
		return core.Session{}, fmt.Errorf("get session %q: %w", id, err)
	}
	sess.ID = core.SessionID(idStr)
	sess.ParentID = core.SessionID(parentID)
	sess.Status = core.SessionStatus(status)
	sess.Owner = core.SessionID(owner)
	sess.WebSearch = webSearch != 0
	sess.WebFetch = webFetch != 0
	sess.ImageGen = imageGen != 0
	sess.CreatedAt = time.Unix(0, createdAt).UTC()
	sess.UpdatedAt = time.Unix(0, updated).UTC()
	return sess, nil
}

func (s *Store) UpdateSession(ctx context.Context, sess core.Session) error {
	s.writeMu.Lock()
	defer s.writeMu.Unlock()
	res, err := s.db.ExecContext(ctx,
		`UPDATE sessions SET parent_id=?, status=?, model=?, principal=?, origin=?, owner=?, spawned_by=?, web_search=?, web_fetch=?, image_gen=?, updated_at=?
		 WHERE id=?`,
		string(sess.ParentID), string(sess.Status), sess.Model,
		sess.Principal, sess.Origin, string(sess.Owner), sess.SpawnedBy, boolToInt(sess.WebSearch), boolToInt(sess.WebFetch), boolToInt(sess.ImageGen),
		sess.UpdatedAt.UnixNano(), string(sess.ID),
	)
	if err != nil {
		return fmt.Errorf("update session %q: %w", sess.ID, err)
	}
	n, err := res.RowsAffected()
	if err != nil {
		return fmt.Errorf("update session %q: %w", sess.ID, err)
	}
	if n == 0 {
		return fmt.Errorf("update session %q: %w", sess.ID, ports.ErrSessionNotFound)
	}
	return nil
}

func (s *Store) AppendEvent(ctx context.Context, ev *core.Event) error {
	if err := ev.Validate(); err != nil {
		return fmt.Errorf("append event: %w", err)
	}
	payload, err := core.EncodePayload(ev.Payload)
	if err != nil {
		return fmt.Errorf("append event: encode payload: %w", err)
	}
	kind := ev.Payload.Kind()

	s.writeMu.Lock()
	defer s.writeMu.Unlock()

	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return fmt.Errorf("append event: begin: %w", err)
	}
	defer func() { _ = tx.Rollback() }()

	// The session must exist (matches the in-memory store and gives a clean
	// error rather than a raw FK violation).
	var exists int
	if err := tx.QueryRowContext(ctx, `SELECT 1 FROM sessions WHERE id = ?`, string(ev.SessionID)).Scan(&exists); err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return fmt.Errorf("append event: %w", ports.ErrSessionNotFound)
		}
		return fmt.Errorf("append event: check session: %w", err)
	}

	var seq int64
	if err := tx.QueryRowContext(ctx,
		`SELECT COALESCE(MAX(seq)+1, 0) FROM events WHERE session_id = ?`, string(ev.SessionID)).Scan(&seq); err != nil {
		return fmt.Errorf("append event: next seq: %w", err)
	}

	res, err := tx.ExecContext(ctx,
		`INSERT INTO events (session_id, seq, turn_id, kind, version, created_at, payload)
		 VALUES (?, ?, ?, ?, ?, ?, ?)`,
		string(ev.SessionID), seq, ev.TurnID, string(kind), ev.Version, ev.CreatedAt.UnixNano(), payload,
	)
	if err != nil {
		return fmt.Errorf("append event: insert: %w", err)
	}
	id, err := res.LastInsertId()
	if err != nil {
		return fmt.Errorf("append event: last id: %w", err)
	}

	if text := searchableText(ev.Payload); text != "" {
		if _, err := tx.ExecContext(ctx,
			`INSERT INTO events_fts (content, session_id, event_id) VALUES (?, ?, ?)`,
			text, string(ev.SessionID), id); err != nil {
			return fmt.Errorf("append event: index fts: %w", err)
		}
	}

	if err := tx.Commit(); err != nil {
		return fmt.Errorf("append event: commit: %w", err)
	}

	ev.ID = id
	ev.Seq = seq
	return nil
}

func (s *Store) Events(ctx context.Context, id core.SessionID) ([]core.Event, error) {
	return s.EventsFrom(ctx, id, 0)
}

// EventsFrom returns a session's events with Seq >= from, oldest first — the
// cursor-read a live mirror (the Telegram bridge) polls with, so it never
// re-reads a long log to see what's new. from = 0 is the full replay (seqs
// are store-assigned starting at 0).
func (s *Store) EventsFrom(ctx context.Context, id core.SessionID, from int64) ([]core.Event, error) {
	rows, err := s.db.QueryContext(ctx,
		`SELECT id, seq, turn_id, kind, version, created_at, payload
		 FROM events WHERE session_id = ? AND seq >= ? ORDER BY seq ASC`, string(id), from)
	if err != nil {
		return nil, fmt.Errorf("events %q: %w", id, err)
	}
	defer func() { _ = rows.Close() }()

	var out []core.Event
	for rows.Next() {
		var (
			ev        core.Event
			kind      string
			createdAt int64
			payload   []byte
		)
		if err := rows.Scan(&ev.ID, &ev.Seq, &ev.TurnID, &kind, &ev.Version, &createdAt, &payload); err != nil {
			return nil, fmt.Errorf("events %q: scan: %w", id, err)
		}
		p, err := core.DecodePayload(core.EventKind(kind), payload)
		if err != nil {
			return nil, fmt.Errorf("events %q: decode seq %d: %w", id, ev.Seq, err)
		}
		ev.SessionID = id
		ev.CreatedAt = time.Unix(0, createdAt).UTC()
		ev.Payload = p
		// Forward-only migration on read: a row written by an older binary is
		// walked up to the current schema before the engine ever sees it.
		ev, err = core.Upcast(ev)
		if err != nil {
			return nil, fmt.Errorf("events %q: upcast seq %d: %w", id, ev.Seq, err)
		}
		out = append(out, ev)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("events %q: %w", id, err)
	}
	return out, nil
}

// SessionSummary is one row of the front-door session list: enough to render
// a history picker without loading event logs.
type SessionSummary struct {
	ID        core.SessionID
	Title     string // first user prompt, first line; "" when no turn ran
	UpdatedAt time.Time
}

const sessionTitleMax = 80

// ListSessions returns top-level human sessions (delegated children and
// scheduler-spawned executors are internal), most recently active first,
// titled by their first user prompt. It is not part of ports.SessionStore —
// it is a frontend listing capability, like Search. Machine-spawned sessions
// carry an owner; the origin filter additionally hides legacy scheduler rows
// from before the owner column.
func (s *Store) ListSessions(ctx context.Context, limit int) ([]SessionSummary, error) {
	rows, err := s.db.QueryContext(ctx, `
		SELECT s.id, s.updated_at,
		       (SELECT e.payload FROM events e
		        WHERE e.session_id = s.id AND e.kind = 'user_message'
		        ORDER BY e.seq ASC LIMIT 1)
		FROM sessions s
		WHERE s.parent_id = '' AND s.owner = '' AND s.origin NOT LIKE ? || '%'
		ORDER BY s.updated_at DESC
		LIMIT ?`, core.OriginScheduler, limit)
	if err != nil {
		return nil, fmt.Errorf("list sessions: %w", err)
	}
	defer func() { _ = rows.Close() }()

	var out []SessionSummary
	for rows.Next() {
		var (
			id      string
			updated int64
			payload []byte
		)
		if err := rows.Scan(&id, &updated, &payload); err != nil {
			return nil, fmt.Errorf("list sessions: scan: %w", err)
		}
		sum := SessionSummary{
			ID:        core.SessionID(id),
			UpdatedAt: time.Unix(0, updated).UTC(),
		}
		sum.Title = titleFromUserMessage(payload)
		// A session nobody ever prompted (an opened-then-abandoned connection)
		// has nothing to resume; keep it out of the picker.
		if sum.Title == "" {
			continue
		}
		out = append(out, sum)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("list sessions: %w", err)
	}
	return out, nil
}

// SessionIDs returns every session id in the store, oldest first. It exists
// for bulk trajectory export (`arbos export --all`), which — unlike the
// frontend picker (ListSessions) — must include machine-spawned children,
// scheduler runs, and untitled sessions: those are rollouts too.
func (s *Store) SessionIDs(ctx context.Context) ([]core.SessionID, error) {
	rows, err := s.db.QueryContext(ctx, `SELECT id FROM sessions ORDER BY created_at ASC`)
	if err != nil {
		return nil, fmt.Errorf("session ids: %w", err)
	}
	defer func() { _ = rows.Close() }()

	var out []core.SessionID
	for rows.Next() {
		var id string
		if err := rows.Scan(&id); err != nil {
			return nil, fmt.Errorf("session ids: scan: %w", err)
		}
		out = append(out, core.SessionID(id))
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("session ids: %w", err)
	}
	return out, nil
}

// ChildSession is one machine-spawned session belonging to a chat: a
// scheduler wake fired for a plan node that chat created. SpawnedBy carries
// the provenance ("node:12"); Status is live while the wake runs.
type ChildSession struct {
	ID        core.SessionID
	SpawnedBy string
	Status    core.SessionStatus
	CreatedAt time.Time
	UpdatedAt time.Time
}

const childSessionLimit = 100

// OwnedSession is one machine-spawned session with its owning chat — a row
// of the cross-chat activity feed.
type OwnedSession struct {
	ID        core.SessionID
	Owner     core.SessionID
	SpawnedBy string
	Origin    string
	Status    core.SessionStatus
	UpdatedAt time.Time
}

// RecentOwnedSessions lists the latest machine-spawned sessions across every
// chat, newest first — the "what has the agent been doing on its own" half of
// the activity surface (the standing-obligation half comes from the plan).
func (s *Store) RecentOwnedSessions(ctx context.Context, limit int) ([]OwnedSession, error) {
	rows, err := s.db.QueryContext(ctx, `
		SELECT id, owner, spawned_by, origin, status, updated_at
		FROM sessions
		WHERE owner <> ''
		ORDER BY updated_at DESC
		LIMIT ?`, limit)
	if err != nil {
		return nil, fmt.Errorf("recent owned sessions: %w", err)
	}
	defer func() { _ = rows.Close() }()

	var out []OwnedSession
	for rows.Next() {
		var (
			o          OwnedSession
			id, owner  string
			status     string
			updatedRaw int64
		)
		if err := rows.Scan(&id, &owner, &o.SpawnedBy, &o.Origin, &status, &updatedRaw); err != nil {
			return nil, fmt.Errorf("recent owned sessions: scan: %w", err)
		}
		o.ID = core.SessionID(id)
		o.Owner = core.SessionID(owner)
		o.Status = core.SessionStatus(status)
		o.UpdatedAt = time.Unix(0, updatedRaw).UTC()
		out = append(out, o)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("recent owned sessions: %w", err)
	}
	return out, nil
}

// RecentSession is one row of the unified cross-session index the agent's
// sessions tool reads: every session that ran at least one prompt — the user's
// other chats and the agent's own autonomous runs — labeled by its first user
// message so the agent can recognize "that Ralph loop I just ran" without
// loading any transcript. Kind is derived: a session with an owner is a machine
// "run" (a scheduler wake), one without is a human "chat".
type RecentSession struct {
	ID        core.SessionID
	Kind      string // "chat" or "run"
	Title     string
	Owner     core.SessionID
	Origin    string
	SpawnedBy string
	Status    core.SessionStatus
	UpdatedAt time.Time
}

// RecentSessions lists the most recently active sessions across every chat and
// run, newest first, each titled by its first user prompt. Unlike ListSessions
// (human chats only) and RecentOwnedSessions (runs only), this is the whole
// picture the agent draws on to find another session by what it was — sessions
// nobody ever prompted are skipped (nothing to review).
func (s *Store) RecentSessions(ctx context.Context, limit int) ([]RecentSession, error) {
	if limit <= 0 {
		limit = 20
	}
	rows, err := s.db.QueryContext(ctx, `
		SELECT s.id, s.owner, s.origin, s.spawned_by, s.status, s.updated_at,
		       (SELECT e.payload FROM events e
		        WHERE e.session_id = s.id AND e.kind = 'user_message'
		        ORDER BY e.seq ASC LIMIT 1)
		FROM sessions s
		ORDER BY s.updated_at DESC
		LIMIT ?`, limit)
	if err != nil {
		return nil, fmt.Errorf("recent sessions: %w", err)
	}
	defer func() { _ = rows.Close() }()

	var out []RecentSession
	for rows.Next() {
		var (
			rs                RecentSession
			id, owner, status string
			updatedRaw        int64
			payload           []byte
		)
		if err := rows.Scan(&id, &owner, &rs.Origin, &rs.SpawnedBy, &status, &updatedRaw, &payload); err != nil {
			return nil, fmt.Errorf("recent sessions: scan: %w", err)
		}
		rs.Title = titleFromUserMessage(payload)
		// A session nobody ever prompted has nothing to review; skip it.
		if rs.Title == "" {
			continue
		}
		rs.ID = core.SessionID(id)
		rs.Owner = core.SessionID(owner)
		rs.Status = core.SessionStatus(status)
		rs.UpdatedAt = time.Unix(0, updatedRaw).UTC()
		rs.Kind = "chat"
		if owner != "" {
			rs.Kind = "run"
		}
		out = append(out, rs)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("recent sessions: %w", err)
	}
	return out, nil
}

// ScheduledChildren lists the scheduler-spawned sessions owned by one chat,
// newest first — the runs a frontend offers as openable sub-agent tabs.
// Scoped by the owner column, so one chat's runs never appear in another.
func (s *Store) ScheduledChildren(ctx context.Context, chat string) ([]ChildSession, error) {
	rows, err := s.db.QueryContext(ctx, `
		SELECT id, spawned_by, status, created_at, updated_at
		FROM sessions
		WHERE owner = ? AND origin = ?
		ORDER BY updated_at DESC
		LIMIT ?`,
		chat, core.OriginScheduler, childSessionLimit)
	if err != nil {
		return nil, fmt.Errorf("scheduled children: %w", err)
	}
	defer func() { _ = rows.Close() }()

	var out []ChildSession
	for rows.Next() {
		var (
			c                ChildSession
			id, status       string
			created, updated int64
		)
		if err := rows.Scan(&id, &c.SpawnedBy, &status, &created, &updated); err != nil {
			return nil, fmt.Errorf("scheduled children: scan: %w", err)
		}
		c.ID = core.SessionID(id)
		c.Status = core.SessionStatus(status)
		c.CreatedAt = time.Unix(0, created).UTC()
		c.UpdatedAt = time.Unix(0, updated).UTC()
		out = append(out, c)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("scheduled children: %w", err)
	}
	return out, nil
}

// titleFromUserMessage decodes a stored user_message payload into a one-line
// list title, or "" when the payload is absent or not a message. Shared by the
// session-listing queries so the three-level decode lives in one place.
func titleFromUserMessage(payload []byte) string {
	if len(payload) == 0 {
		return ""
	}
	p, err := core.DecodePayload(core.EventUserMessage, payload)
	if err != nil {
		return ""
	}
	mp, ok := p.(core.MessagePayload)
	if !ok {
		return ""
	}
	return sessionTitle(mp.Message.Content)
}

// sessionTitle reduces a prompt to one list-row line.
func sessionTitle(prompt string) string {
	line := prompt
	if i := strings.IndexByte(line, '\n'); i >= 0 {
		line = line[:i]
	}
	line = strings.TrimSpace(line)
	if len(line) > sessionTitleMax {
		line = line[:sessionTitleMax] + "…"
	}
	return line
}

// SearchHit is one FTS5 match: the matching event and its session.
type SearchHit struct {
	SessionID core.SessionID
	EventID   int64
	Snippet   string
}

// Search runs an FTS5 query over message/context text across all sessions (or
// one, when sessionID is non-empty). It is not part of ports.SessionStore — it
// is a SQLite-specific capability the higher layers (session search, memory
// retrieval) use directly. Results are ordered by relevance.
func (s *Store) Search(ctx context.Context, sessionID core.SessionID, query string, limit int) ([]SearchHit, error) {
	if limit <= 0 {
		limit = 20
	}
	q := `SELECT session_id, event_id, snippet(events_fts, 0, '[', ']', '...', 12)
	      FROM events_fts WHERE events_fts MATCH ?`
	args := []any{query}
	if sessionID != "" {
		q += ` AND session_id = ?`
		args = append(args, string(sessionID))
	}
	q += ` ORDER BY rank LIMIT ?`
	args = append(args, limit)

	rows, err := s.db.QueryContext(ctx, q, args...)
	if err != nil {
		return nil, fmt.Errorf("search: %w", err)
	}
	defer func() { _ = rows.Close() }()

	var hits []SearchHit
	for rows.Next() {
		var (
			h   SearchHit
			sid string
		)
		if err := rows.Scan(&sid, &h.EventID, &h.Snippet); err != nil {
			return nil, fmt.Errorf("search: scan: %w", err)
		}
		h.SessionID = core.SessionID(sid)
		hits = append(hits, h)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("search: %w", err)
	}
	return hits, nil
}

// searchableText extracts the human text worth indexing for FTS from a payload.
// Only message and injected-context kinds carry searchable prose; everything
// else returns "" and is not indexed.
func searchableText(p core.EventPayload) string {
	switch v := p.(type) {
	case core.MessagePayload:
		return v.Message.Content
	case core.ContextPayload:
		var b []byte
		for i, seg := range v.Segments {
			if i > 0 {
				b = append(b, '\n')
			}
			b = append(b, seg.Content...)
		}
		return string(b)
	case core.ToolResultPayload, core.UsagePayload, core.CompressionPayload, core.InterruptPayload:
		return ""
	default:
		return ""
	}
}

// boolToInt maps a Go bool to SQLite's 0/1 INTEGER convention for boolean
// columns (the driver has no native bool type).
func boolToInt(b bool) int {
	if b {
		return 1
	}
	return 0
}

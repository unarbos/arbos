package sqlite

import (
	"context"
	"crypto/rand"
	"database/sql"
	"encoding/hex"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/unarbos/arbos/internal/core"
)

// This file is the storage half of the agent's memory (the "mind"): durable
// atoms in one global set plus per-session curation checkpoints. It holds no
// intelligence — distillation and recall ranking live in internal/mind. These
// methods are SQLite-specific capabilities (like Search), not part of
// ports.SessionStore.

// RecallAtoms returns atoms whose content matches the FTS query, most relevant
// first. An empty/blank query falls back to the most recently updated atoms so
// recall still surfaces something on a bare prompt.
func (s *Store) RecallAtoms(ctx context.Context, match string, limit int) ([]core.Atom, error) {
	if limit <= 0 {
		limit = 12
	}
	if match == "" {
		return s.AllAtoms(ctx, limit)
	}
	rows, err := s.db.QueryContext(ctx,
		`SELECT a.id, a.content, a.updated_at
		   FROM atoms_fts f JOIN atoms a ON a.id = f.atom_id
		  WHERE atoms_fts MATCH ?
		  ORDER BY rank LIMIT ?`, match, limit)
	if err != nil {
		return nil, fmt.Errorf("recall atoms: %w", err)
	}
	return scanAtoms(rows)
}

// AllAtoms returns the most recently updated atoms, newest first. The curator
// uses it to see existing memory so it can merge (reuse ids) rather than
// duplicate.
func (s *Store) AllAtoms(ctx context.Context, limit int) ([]core.Atom, error) {
	if limit <= 0 {
		limit = 100
	}
	rows, err := s.db.QueryContext(ctx,
		`SELECT id, content, updated_at FROM atoms ORDER BY updated_at DESC LIMIT ?`, limit)
	if err != nil {
		return nil, fmt.Errorf("all atoms: %w", err)
	}
	return scanAtoms(rows)
}

// AtomCheckpoint returns the last event seq folded into atoms for a session
// (-1 if none yet, so a brand-new session curates from seq 0).
func (s *Store) AtomCheckpoint(ctx context.Context, sid core.SessionID) (int64, error) {
	var seq int64
	err := s.db.QueryRowContext(ctx,
		`SELECT last_seq FROM mind_checkpoints WHERE session_id = ?`, string(sid)).Scan(&seq)
	if err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return -1, nil
		}
		return -1, fmt.Errorf("atom checkpoint: %w", err)
	}
	return seq, nil
}

// AddAtom writes one atom the agent chose to remember — the explicit,
// deliberate write that complements background curation. It assigns a fresh id
// (curation may later merge it with similar atoms by content); the agent need
// not manage ids. Insert + FTS index in one statement pair, under the write
// lock, like CommitCuration's per-atom write.
func (s *Store) AddAtom(ctx context.Context, content string) error {
	content = strings.TrimSpace(content)
	if content == "" {
		return fmt.Errorf("add atom: empty content")
	}
	s.writeMu.Lock()
	defer s.writeMu.Unlock()
	var b [8]byte
	_, _ = rand.Read(b[:])
	id := "mem-" + hex.EncodeToString(b[:])
	now := time.Now().UnixNano()
	if _, err := s.db.ExecContext(ctx,
		`INSERT INTO atoms (id, content, updated_at) VALUES (?, ?, ?)`, id, content, now); err != nil {
		return fmt.Errorf("add atom: %w", err)
	}
	if _, err := s.db.ExecContext(ctx,
		`INSERT INTO atoms_fts (content, atom_id) VALUES (?, ?)`, content, id); err != nil {
		return fmt.Errorf("add atom: fts: %w", err)
	}
	return nil
}

// CommitCuration applies one curation pass atomically: upsert each set atom,
// delete each forgotten id, and advance the session checkpoint — all in a single
// transaction so a crash never advances the checkpoint past unwritten atoms.
func (s *Store) CommitCuration(ctx context.Context, sid core.SessionID, set []core.Atom, forget []string, newSeq int64) error {
	s.writeMu.Lock()
	defer s.writeMu.Unlock()

	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return fmt.Errorf("curation: begin: %w", err)
	}
	defer func() { _ = tx.Rollback() }()

	now := time.Now().UnixNano()
	for _, a := range set {
		if a.ID == "" || a.Content == "" {
			continue
		}
		if _, err := tx.ExecContext(ctx,
			`INSERT INTO atoms (id, content, updated_at) VALUES (?, ?, ?)
			 ON CONFLICT(id) DO UPDATE SET content=excluded.content, updated_at=excluded.updated_at`,
			a.ID, a.Content, now); err != nil {
			return fmt.Errorf("curation: upsert %q: %w", a.ID, err)
		}
		if _, err := tx.ExecContext(ctx, `DELETE FROM atoms_fts WHERE atom_id = ?`, a.ID); err != nil {
			return fmt.Errorf("curation: fts clear %q: %w", a.ID, err)
		}
		if _, err := tx.ExecContext(ctx,
			`INSERT INTO atoms_fts (content, atom_id) VALUES (?, ?)`, a.Content, a.ID); err != nil {
			return fmt.Errorf("curation: fts index %q: %w", a.ID, err)
		}
	}
	for _, id := range forget {
		if id == "" {
			continue
		}
		if _, err := tx.ExecContext(ctx, `DELETE FROM atoms WHERE id = ?`, id); err != nil {
			return fmt.Errorf("curation: forget %q: %w", id, err)
		}
		if _, err := tx.ExecContext(ctx, `DELETE FROM atoms_fts WHERE atom_id = ?`, id); err != nil {
			return fmt.Errorf("curation: fts forget %q: %w", id, err)
		}
	}
	if _, err := tx.ExecContext(ctx,
		`INSERT INTO mind_checkpoints (session_id, last_seq) VALUES (?, ?)
		 ON CONFLICT(session_id) DO UPDATE SET last_seq=excluded.last_seq`,
		string(sid), newSeq); err != nil {
		return fmt.Errorf("curation: checkpoint: %w", err)
	}

	if err := tx.Commit(); err != nil {
		return fmt.Errorf("curation: commit: %w", err)
	}
	return nil
}

func scanAtoms(rows *sql.Rows) ([]core.Atom, error) {
	defer func() { _ = rows.Close() }()
	var out []core.Atom
	for rows.Next() {
		var (
			a       core.Atom
			updated int64
		)
		if err := rows.Scan(&a.ID, &a.Content, &updated); err != nil {
			return nil, fmt.Errorf("scan atom: %w", err)
		}
		a.UpdatedAt = time.Unix(0, updated).UTC()
		out = append(out, a)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("scan atoms: %w", err)
	}
	return out, nil
}

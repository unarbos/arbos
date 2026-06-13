package sqlite

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"time"

	"github.com/unarbos/arbos/internal/board"
)

// ErrNoBoard is returned by GetBoard when an id resolves to no row.
var ErrNoBoard = errors.New("no such board")

// PutBoard inserts or updates a board by id. created_at is set on first insert
// and preserved across updates (ON CONFLICT leaves it untouched), so re-saving
// a board keeps its original creation time while bumping updated_at.
func (s *Store) PutBoard(ctx context.Context, b board.Board) error {
	members := []byte("[]")
	if len(b.Members) > 0 {
		if enc, err := json.Marshal(b.Members); err == nil {
			members = enc
		}
	}
	s.writeMu.Lock()
	defer s.writeMu.Unlock()
	_, err := s.db.ExecContext(ctx, `
INSERT INTO boards (id, title, layout, members, created_at, updated_at)
VALUES (?, ?, ?, ?, ?, ?)
ON CONFLICT(id) DO UPDATE SET
    title = excluded.title,
    layout = excluded.layout,
    members = excluded.members,
    updated_at = excluded.updated_at`,
		b.ID, b.Title, []byte(b.Layout), members,
		b.CreatedAt.UnixNano(), b.UpdatedAt.UnixNano(),
	)
	if err != nil {
		return fmt.Errorf("put board %q: %w", b.ID, err)
	}
	return nil
}

// GetBoard loads a board by id, or ErrNoBoard when absent.
func (s *Store) GetBoard(ctx context.Context, id string) (board.Board, error) {
	row := s.db.QueryRowContext(ctx,
		`SELECT id, title, layout, members, created_at, updated_at FROM boards WHERE id = ?`, id)
	var (
		b                board.Board
		layout, members  []byte
		created, updated int64
	)
	err := row.Scan(&b.ID, &b.Title, &layout, &members, &created, &updated)
	if errors.Is(err, sql.ErrNoRows) {
		return board.Board{}, ErrNoBoard
	}
	if err != nil {
		return board.Board{}, fmt.Errorf("board %q: %w", id, err)
	}
	b.Layout = json.RawMessage(layout)
	if len(members) > 0 {
		if err := json.Unmarshal(members, &b.Members); err != nil {
			return board.Board{}, fmt.Errorf("board %q members: %w", id, err)
		}
	}
	b.CreatedAt = time.Unix(0, created)
	b.UpdatedAt = time.Unix(0, updated)
	return b, nil
}

// BoardSummary is one row of the board list: enough to render a picker without
// loading every layout blob.
type BoardSummary struct {
	ID        string
	Title     string
	UpdatedAt time.Time
}

// ListBoards returns board summaries, most-recently-updated first, capped at
// limit.
func (s *Store) ListBoards(ctx context.Context, limit int) ([]BoardSummary, error) {
	rows, err := s.db.QueryContext(ctx,
		`SELECT id, title, updated_at FROM boards ORDER BY updated_at DESC LIMIT ?`, limit)
	if err != nil {
		return nil, fmt.Errorf("list boards: %w", err)
	}
	defer func() { _ = rows.Close() }()
	var out []BoardSummary
	for rows.Next() {
		var (
			sum     BoardSummary
			updated int64
		)
		if err := rows.Scan(&sum.ID, &sum.Title, &updated); err != nil {
			return nil, fmt.Errorf("list boards: %w", err)
		}
		sum.UpdatedAt = time.Unix(0, updated)
		out = append(out, sum)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("list boards: %w", err)
	}
	return out, nil
}

// DeleteBoard removes a board by id. Deleting an absent board is a no-op
// success.
func (s *Store) DeleteBoard(ctx context.Context, id string) error {
	s.writeMu.Lock()
	defer s.writeMu.Unlock()
	if _, err := s.db.ExecContext(ctx, `DELETE FROM boards WHERE id = ?`, id); err != nil {
		return fmt.Errorf("delete board %q: %w", id, err)
	}
	return nil
}

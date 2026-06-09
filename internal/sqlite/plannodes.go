package sqlite

import (
	"context"
	"database/sql"
	"fmt"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/plan"
)

// This file is the storage half of the agent's intent (the "plan"): the
// durable goal forest and its append-only attempt history. It holds no
// intelligence — transitions, readiness, and rendering live in internal/plan.
// These methods are SQLite-specific capabilities (like Search and the atom
// methods), not part of ports.SessionStore.

// AddPlanNodes appends nodes in order. parent == 0 starts a new plan: the
// first node becomes the root (its plan_id is its own id) and the rest its
// children. Otherwise all nodes append as children of parent, after its
// existing children. IDs are returned in input order.
func (s *Store) AddPlanNodes(ctx context.Context, parent plan.NodeID, nodes []plan.Node) ([]plan.NodeID, error) {
	if len(nodes) == 0 {
		return nil, fmt.Errorf("add plan nodes: no nodes")
	}
	s.writeMu.Lock()
	defer s.writeMu.Unlock()

	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return nil, fmt.Errorf("add plan nodes: begin: %w", err)
	}
	defer func() { _ = tx.Rollback() }()

	now := time.Now().UnixNano()
	rest := nodes
	var ids []plan.NodeID

	if parent == 0 {
		root := nodes[0]
		id, err := insertPlanNode(ctx, tx, 0, 0, 0, root, now)
		if err != nil {
			return nil, err
		}
		// A root's plan is itself; the id only exists after insert.
		if _, err := tx.ExecContext(ctx, `UPDATE plan_nodes SET plan_id = ? WHERE id = ?`, int64(id), int64(id)); err != nil {
			return nil, fmt.Errorf("add plan nodes: set root plan: %w", err)
		}
		ids = append(ids, id)
		parent = id
		rest = nodes[1:]
	}

	parentRow, err := planNodeTx(ctx, tx, parent)
	if err != nil {
		return nil, err
	}
	// Sibling order is the dependency relation, and equal seq is the parallel
	// group (GatedBySibling gates on strictly-lower seq only). Par appends a
	// node at the PREVIOUS node's slot — for the batch's first node, the last
	// existing sibling's — so a fan-out reads 1,1,1,2: three alongside, then
	// the join.
	var maxSeq sql.NullInt64
	if err := tx.QueryRowContext(ctx,
		`SELECT MAX(seq) FROM plan_nodes WHERE parent_id = ?`, int64(parent)).Scan(&maxSeq); err != nil {
		return nil, fmt.Errorf("add plan nodes: next seq: %w", err)
	}
	next := 0
	prev := -1
	if maxSeq.Valid {
		next = int(maxSeq.Int64) + 1
		prev = int(maxSeq.Int64)
	}
	for _, n := range rest {
		seq := next
		if n.Par && prev >= 0 {
			seq = prev
		}
		id, err := insertPlanNode(ctx, tx, parentRow.Plan, parent, seq, n, now)
		if err != nil {
			return nil, err
		}
		ids = append(ids, id)
		prev = seq
		next = seq + 1
	}

	if err := tx.Commit(); err != nil {
		return nil, fmt.Errorf("add plan nodes: commit: %w", err)
	}
	return ids, nil
}

func insertPlanNode(ctx context.Context, tx *sql.Tx, planID, parent plan.NodeID, seq int, n plan.Node, now int64) (plan.NodeID, error) {
	res, err := tx.ExecContext(ctx,
		`INSERT INTO plan_nodes
		   (plan_id, parent_id, seq, kind, goal, check_expr, cmd, wake, status, outcome, assignee, owner,
		    after_at, every_ns, next_due, created_at, updated_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
		int64(planID), int64(parent), seq, string(n.Kind), n.Goal, n.Check, n.Cmd, n.WakeOnReady, string(n.Status),
		n.Outcome, n.Assignee, n.Owner, nanos(n.After), int64(n.Every), nanos(n.NextDue), now, now)
	if err != nil {
		return 0, fmt.Errorf("add plan nodes: insert: %w", err)
	}
	id, err := res.LastInsertId()
	if err != nil {
		return 0, fmt.Errorf("add plan nodes: id: %w", err)
	}
	return plan.NodeID(id), nil
}

// PlanNode fetches one node by id.
func (s *Store) PlanNode(ctx context.Context, id plan.NodeID) (plan.Node, error) {
	return scanPlanNode(s.db.QueryRowContext(ctx, planNodeSelect+` WHERE id = ?`, int64(id)))
}

func planNodeTx(ctx context.Context, tx *sql.Tx, id plan.NodeID) (plan.Node, error) {
	return scanPlanNode(tx.QueryRowContext(ctx, planNodeSelect+` WHERE id = ?`, int64(id)))
}

// UpdatePlanNode persists a node's mutable fields by ID. The caller
// (internal/plan) has already validated the transition; storage stays dumb.
func (s *Store) UpdatePlanNode(ctx context.Context, n plan.Node) error {
	s.writeMu.Lock()
	defer s.writeMu.Unlock()
	res, err := s.db.ExecContext(ctx,
		`UPDATE plan_nodes
		    SET kind=?, goal=?, check_expr=?, cmd=?, wake=?, status=?, outcome=?, assignee=?, owner=?,
		        after_at=?, every_ns=?, next_due=?, updated_at=?
		  WHERE id = ?`,
		string(n.Kind), n.Goal, n.Check, n.Cmd, n.WakeOnReady, string(n.Status), n.Outcome, n.Assignee, n.Owner,
		nanos(n.After), int64(n.Every), nanos(n.NextDue), time.Now().UnixNano(), int64(n.ID))
	if err != nil {
		return fmt.Errorf("update plan node %d: %w", n.ID, err)
	}
	if rows, _ := res.RowsAffected(); rows == 0 {
		return fmt.Errorf("update plan node %d: not found", n.ID)
	}
	return nil
}

// OpenPlanNodes returns every node of every plan whose root is not terminal,
// ordered (plan, parent, seq) — the working forest the projection renders.
func (s *Store) OpenPlanNodes(ctx context.Context) ([]plan.Node, error) {
	rows, err := s.db.QueryContext(ctx,
		planNodeSelectN+`
		   JOIN plan_nodes r ON r.id = n.plan_id
		  WHERE r.status NOT IN ('done','cancelled','failed')
		  ORDER BY n.plan_id, n.parent_id, n.seq`)
	if err != nil {
		return nil, fmt.Errorf("open plan nodes: %w", err)
	}
	defer func() { _ = rows.Close() }()
	var out []plan.Node
	for rows.Next() {
		n, err := scanPlanNode(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, n)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("open plan nodes: %w", err)
	}
	return out, nil
}

// ClaimPlanNode atomically moves a pending node to active for owner. The
// WHERE status='pending' is the compare half of compare-and-claim: when
// several scheduler processes share this store, exactly one caller sees a
// row change and wins the node.
func (s *Store) ClaimPlanNode(ctx context.Context, id plan.NodeID, owner string) (bool, error) {
	s.writeMu.Lock()
	defer s.writeMu.Unlock()
	res, err := s.db.ExecContext(ctx,
		`UPDATE plan_nodes SET status='active', owner=?, updated_at=? WHERE id=? AND status='pending'`,
		owner, time.Now().UnixNano(), int64(id))
	if err != nil {
		return false, fmt.Errorf("claim plan node %d: %w", id, err)
	}
	rows, err := res.RowsAffected()
	if err != nil {
		return false, fmt.Errorf("claim plan node %d: %w", id, err)
	}
	return rows > 0, nil
}

// DisarmPlanNode atomically replaces a node's triggers (time arming and the
// one-shot wake flag), but only if they still match what the caller read —
// compare-and-disarm, so a firing happens once per arming no matter how many
// schedulers tick.
func (s *Store) DisarmPlanNode(ctx context.Context, n plan.Node, after, nextDue time.Time, wakeOnReady bool) (bool, error) {
	s.writeMu.Lock()
	defer s.writeMu.Unlock()
	res, err := s.db.ExecContext(ctx,
		`UPDATE plan_nodes SET after_at=?, next_due=?, wake=?, updated_at=?
		  WHERE id=? AND after_at=? AND next_due=? AND wake=? AND status='pending'`,
		nanos(after), nanos(nextDue), wakeOnReady, time.Now().UnixNano(),
		int64(n.ID), nanos(n.After), nanos(n.NextDue), n.WakeOnReady)
	if err != nil {
		return false, fmt.Errorf("disarm plan node %d: %w", n.ID, err)
	}
	rows, err := res.RowsAffected()
	if err != nil {
		return false, fmt.Errorf("disarm plan node %d: %w", n.ID, err)
	}
	return rows > 0, nil
}

// AddPlanAttempt appends one attempt record. Attempts are never updated or
// deleted — failed attempts are knowledge.
func (s *Store) AddPlanAttempt(ctx context.Context, a plan.Attempt) error {
	s.writeMu.Lock()
	defer s.writeMu.Unlock()
	_, err := s.db.ExecContext(ctx,
		`INSERT INTO plan_attempts (node_id, session_id, verdict, outcome, verified_by, workspace, created_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?)`,
		int64(a.Node), a.Session, string(a.Verdict), a.Outcome, a.VerifiedBy, a.Workspace, time.Now().UnixNano())
	if err != nil {
		return fmt.Errorf("add plan attempt: %w", err)
	}
	return nil
}

// LastPlanAttempts returns the most recent attempt per node, for nodes of
// open plans — the working memory the projection shows under each open node
// (a recurrence's previous reading, a retry's failed approach).
func (s *Store) LastPlanAttempts(ctx context.Context) (map[plan.NodeID]plan.Attempt, error) {
	rows, err := s.db.QueryContext(ctx,
		`SELECT a.node_id, a.session_id, a.verdict, a.outcome, a.verified_by, a.workspace, a.created_at
		   FROM plan_attempts a
		   JOIN (SELECT node_id, MAX(id) AS max_id FROM plan_attempts GROUP BY node_id) m ON m.max_id = a.id
		   JOIN plan_nodes n ON n.id = a.node_id
		   JOIN plan_nodes r ON r.id = n.plan_id
		  WHERE r.status NOT IN ('done','cancelled','failed')`)
	if err != nil {
		return nil, fmt.Errorf("last plan attempts: %w", err)
	}
	defer func() { _ = rows.Close() }()
	out := map[plan.NodeID]plan.Attempt{}
	for rows.Next() {
		var (
			a        plan.Attempt
			node, at int64
			verdict  string
		)
		if err := rows.Scan(&node, &a.Session, &verdict, &a.Outcome, &a.VerifiedBy, &a.Workspace, &at); err != nil {
			return nil, fmt.Errorf("scan last plan attempt: %w", err)
		}
		a.Node, a.Verdict, a.At = plan.NodeID(node), plan.Verdict(verdict), fromNanos(at)
		out[a.Node] = a
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("last plan attempts: %w", err)
	}
	return out, nil
}

// PlanNodesUpdatedSince returns nodes updated after since, from any plan —
// including closed ones, since a mission finishing is exactly what the
// front-door brief must report. Ordered by update time, oldest first.
func (s *Store) PlanNodesUpdatedSince(ctx context.Context, since time.Time) ([]plan.Node, error) {
	rows, err := s.db.QueryContext(ctx,
		planNodeSelect+` WHERE updated_at > ? ORDER BY updated_at`, nanos(since))
	if err != nil {
		return nil, fmt.Errorf("plan nodes updated since: %w", err)
	}
	defer func() { _ = rows.Close() }()
	var out []plan.Node
	for rows.Next() {
		n, err := scanPlanNode(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, n)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("plan nodes updated since: %w", err)
	}
	return out, nil
}

// PlanAttemptsSince returns attempts recorded after since, oldest first — the
// front-door brief's "what ran while you were away".
func (s *Store) PlanAttemptsSince(ctx context.Context, since time.Time) ([]plan.Attempt, error) {
	rows, err := s.db.QueryContext(ctx,
		`SELECT node_id, session_id, verdict, outcome, verified_by, workspace, created_at
		   FROM plan_attempts WHERE created_at > ? ORDER BY created_at`, nanos(since))
	if err != nil {
		return nil, fmt.Errorf("plan attempts since: %w", err)
	}
	defer func() { _ = rows.Close() }()
	var out []plan.Attempt
	for rows.Next() {
		var (
			a        plan.Attempt
			node, at int64
			verdict  string
		)
		if err := rows.Scan(&node, &a.Session, &verdict, &a.Outcome, &a.VerifiedBy, &a.Workspace, &at); err != nil {
			return nil, fmt.Errorf("scan plan attempt: %w", err)
		}
		a.Node, a.Verdict, a.At = plan.NodeID(node), plan.Verdict(verdict), fromNanos(at)
		out = append(out, a)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("plan attempts since: %w", err)
	}
	return out, nil
}

// LastHumanSeen returns the time of the most recent event in any session a
// human drove (origin is not the scheduler's) — the "since you left" anchor.
// Zero when no such event exists.
func (s *Store) LastHumanSeen(ctx context.Context) (time.Time, error) {
	var at int64
	err := s.db.QueryRowContext(ctx,
		`SELECT COALESCE(MAX(e.created_at), 0)
		   FROM events e JOIN sessions s ON s.id = e.session_id
		  WHERE s.origin <> ?`, core.OriginScheduler).Scan(&at)
	if err != nil {
		return time.Time{}, fmt.Errorf("last human seen: %w", err)
	}
	return fromNanos(at), nil
}

const planNodeSelect = `SELECT id, plan_id, parent_id, seq, kind, goal, check_expr, cmd, wake, status, outcome,
       assignee, owner, after_at, every_ns, next_due, updated_at FROM plan_nodes`

const planNodeSelectN = `SELECT n.id, n.plan_id, n.parent_id, n.seq, n.kind, n.goal, n.check_expr, n.cmd, n.wake, n.status, n.outcome,
       n.assignee, n.owner, n.after_at, n.every_ns, n.next_due, n.updated_at FROM plan_nodes n`

type rowScanner interface{ Scan(dest ...any) error }

func scanPlanNode(r rowScanner) (plan.Node, error) {
	var (
		n                        plan.Node
		id, planID, parent       int64
		kind, status             string
		after, every, due, updat int64
	)
	if err := r.Scan(&id, &planID, &parent, &n.Seq, &kind, &n.Goal, &n.Check, &n.Cmd, &n.WakeOnReady, &status, &n.Outcome,
		&n.Assignee, &n.Owner, &after, &every, &due, &updat); err != nil {
		return plan.Node{}, fmt.Errorf("scan plan node: %w", err)
	}
	n.ID, n.Plan, n.Parent = plan.NodeID(id), plan.NodeID(planID), plan.NodeID(parent)
	n.Kind, n.Status = plan.Kind(kind), plan.Status(status)
	n.After, n.NextDue = fromNanos(after), fromNanos(due)
	n.Every = time.Duration(every)
	n.UpdatedAt = fromNanos(updat)
	return n, nil
}

// nanos maps a time to its stored representation; the zero time stores as 0
// so "no deferral / not due" round-trips exactly.
func nanos(t time.Time) int64 {
	if t.IsZero() {
		return 0
	}
	return t.UnixNano()
}

func fromNanos(v int64) time.Time {
	if v == 0 {
		return time.Time{}
	}
	return time.Unix(0, v).UTC()
}

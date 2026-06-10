package sqlite_test

import (
	"context"
	"sync"
	"testing"
	"time"

	"github.com/unarbos/arbos/internal/plan"
)

// These tests pin the store's concurrency primitives: the compare-and-claim
// and compare-and-disarm UPDATEs are what make a plan safe to share between an
// interactive session and the serve host, and the narrow status write must not
// disturb the trigger columns the scheduler owns.

func addOneNode(t *testing.T, s interface {
	AddPlanNodes(context.Context, plan.NodeID, []plan.Node, []bool) ([]plan.NodeID, error)
}, n plan.Node) plan.NodeID {
	t.Helper()
	ids, err := s.AddPlanNodes(context.Background(), 0, []plan.Node{n}, []bool{false})
	if err != nil {
		t.Fatalf("add: %v", err)
	}
	return ids[0]
}

func TestClaimPlanNode_SingleWinner(t *testing.T) {
	s := openTemp(t)
	ctx := context.Background()
	id := addOneNode(t, s, plan.Node{Goal: "race", Status: plan.StatusPending, Assignee: plan.AssigneeAgent})

	const racers = 8
	var wins int32
	var mu sync.Mutex
	var wg sync.WaitGroup
	for i := 0; i < racers; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			won, err := s.ClaimPlanNode(ctx, id, "kernel")
			if err != nil {
				t.Errorf("claim: %v", err)
				return
			}
			if won {
				mu.Lock()
				wins++
				mu.Unlock()
			}
		}()
	}
	wg.Wait()
	if wins != 1 {
		t.Fatalf("expected exactly one winner, got %d", wins)
	}
}

func TestDisarmPlanNode_OncePerArming(t *testing.T) {
	s := openTemp(t)
	ctx := context.Background()
	due := time.Now().Add(-time.Minute).UTC()
	id := addOneNode(t, s, plan.Node{Goal: "deferred", Status: plan.StatusPending, Assignee: plan.AssigneeAgent, After: due})

	n, err := s.PlanNode(ctx, id)
	if err != nil {
		t.Fatalf("get: %v", err)
	}
	// First disarm against the read arming wins; a second with the same stale
	// arming loses (the row no longer matches).
	won, err := s.DisarmPlanNode(ctx, n, time.Time{}, time.Time{}, false)
	if err != nil || !won {
		t.Fatalf("first disarm should win: won=%v err=%v", won, err)
	}
	won, err = s.DisarmPlanNode(ctx, n, time.Time{}, time.Time{}, false)
	if err != nil {
		t.Fatalf("second disarm err: %v", err)
	}
	if won {
		t.Fatal("second disarm with stale arming should lose")
	}
}

func TestSetPlanNodeStatus_LeavesTriggersIntact(t *testing.T) {
	s := openTemp(t)
	ctx := context.Background()
	due := time.Now().Add(time.Hour).UTC()
	id := addOneNode(t, s, plan.Node{Goal: "deferred", Status: plan.StatusPending, Assignee: plan.AssigneeAgent, After: due, WakeOnReady: true})

	if err := s.SetPlanNodeStatus(ctx, id, plan.StatusActive, "claimed", "sess-x"); err != nil {
		t.Fatalf("set status: %v", err)
	}
	n, err := s.PlanNode(ctx, id)
	if err != nil {
		t.Fatalf("get: %v", err)
	}
	if n.Status != plan.StatusActive || n.Owner != "sess-x" || n.Outcome != "claimed" {
		t.Fatalf("lifecycle columns not written: %+v", n)
	}
	if n.After.IsZero() || !n.WakeOnReady {
		t.Fatalf("trigger columns were disturbed by a status write: After=%v wake=%v", n.After, n.WakeOnReady)
	}
}

func TestSetPlanNodeStatusIf_CASRejectsStaleFrom(t *testing.T) {
	s := openTemp(t)
	ctx := context.Background()
	id := addOneNode(t, s, plan.Node{Goal: "claim me", Status: plan.StatusPending, Assignee: plan.AssigneeAgent})

	// First claimant (pending->active) wins.
	won, err := s.SetPlanNodeStatusIf(ctx, id, plan.StatusPending, plan.StatusActive, "", "sess-a")
	if err != nil || !won {
		t.Fatalf("first claim should win: won=%v err=%v", won, err)
	}
	// Second claimant expecting pending loses — it is already active.
	won, err = s.SetPlanNodeStatusIf(ctx, id, plan.StatusPending, plan.StatusActive, "", "sess-b")
	if err != nil {
		t.Fatalf("second claim err: %v", err)
	}
	if won {
		t.Fatal("second claim with stale expected-status should lose")
	}
	n, _ := s.PlanNode(ctx, id)
	if n.Owner != "sess-a" {
		t.Fatalf("owner = %q, want sess-a (first winner)", n.Owner)
	}
}

func TestReclaimStaleKernelNodes(t *testing.T) {
	s := openTemp(t)
	ctx := context.Background()

	stale := addOneNode(t, s, plan.Node{Goal: "orphan", Cmd: "true", Status: plan.StatusPending, Assignee: plan.AssigneeAgent})
	humanHeld := addOneNode(t, s, plan.Node{Goal: "human active", Status: plan.StatusPending, Assignee: plan.AssigneeAgent})

	// Kernel-claim the orphan; session-claim the other.
	if _, err := s.ClaimPlanNode(ctx, stale, "kernel"); err != nil {
		t.Fatalf("claim kernel: %v", err)
	}
	if ok, err := s.SetPlanNodeStatusIf(ctx, humanHeld, plan.StatusPending, plan.StatusActive, "", "sess-x"); err != nil || !ok {
		t.Fatalf("claim session: ok=%v err=%v", ok, err)
	}

	// Reclaim everything claimed before "now+1m" — both are recent, so the
	// kernel one matches the age window but the session one is excluded by owner.
	n, err := s.ReclaimStaleKernelNodes(ctx, time.Now().Add(time.Minute))
	if err != nil {
		t.Fatalf("reclaim: %v", err)
	}
	if n != 1 {
		t.Fatalf("reclaimed %d, want 1 (only the kernel-owned orphan)", n)
	}
	got, _ := s.PlanNode(ctx, stale)
	if got.Status != plan.StatusPending || got.Owner != "" {
		t.Fatalf("orphan not reset: status=%s owner=%q", got.Status, got.Owner)
	}
	held, _ := s.PlanNode(ctx, humanHeld)
	if held.Status != plan.StatusActive || held.Owner != "sess-x" {
		t.Fatalf("session-held node disturbed: status=%s owner=%q", held.Status, held.Owner)
	}
}

func TestReclaimStaleKernelNodes_SkipsRecent(t *testing.T) {
	s := openTemp(t)
	ctx := context.Background()
	id := addOneNode(t, s, plan.Node{Goal: "running", Cmd: "true", Status: plan.StatusPending, Assignee: plan.AssigneeAgent})
	if _, err := s.ClaimPlanNode(ctx, id, "kernel"); err != nil {
		t.Fatalf("claim: %v", err)
	}
	// A long-past cutoff: the just-claimed node is newer, so it is NOT stale.
	n, err := s.ReclaimStaleKernelNodes(ctx, time.Now().Add(-time.Hour))
	if err != nil {
		t.Fatalf("reclaim: %v", err)
	}
	if n != 0 {
		t.Fatalf("reclaimed %d, want 0 (claim is recent)", n)
	}
}

func TestAddPlanNodes_ParallelGroupSeqs(t *testing.T) {
	s := openTemp(t)
	ctx := context.Background()
	// root + fan-out(3) + join: par = [_, false, true, true, false].
	nodes := []plan.Node{
		{Goal: "mission", Status: plan.StatusPending, Assignee: plan.AssigneeAgent},
		{Goal: "a", Cmd: "true", Status: plan.StatusPending, Assignee: plan.AssigneeAgent},
		{Goal: "b", Cmd: "true", Status: plan.StatusPending, Assignee: plan.AssigneeAgent},
		{Goal: "c", Cmd: "true", Status: plan.StatusPending, Assignee: plan.AssigneeAgent},
		{Goal: "join", Cmd: "true", Status: plan.StatusPending, Assignee: plan.AssigneeAgent},
	}
	par := []bool{false, false, true, true, false}
	ids, err := s.AddPlanNodes(ctx, 0, nodes, par)
	if err != nil {
		t.Fatalf("add: %v", err)
	}
	open, err := s.OpenPlanNodes(ctx)
	if err != nil {
		t.Fatalf("open: %v", err)
	}
	seq := map[plan.NodeID]int{}
	for _, n := range open {
		seq[n.ID] = n.Seq
	}
	// a, b, c share a seq; join is strictly after.
	if seq[ids[1]] != seq[ids[2]] || seq[ids[2]] != seq[ids[3]] {
		t.Fatalf("fan-out should share a seq: %d %d %d", seq[ids[1]], seq[ids[2]], seq[ids[3]])
	}
	if seq[ids[4]] <= seq[ids[1]] {
		t.Fatalf("join seq %d should be after group seq %d", seq[ids[4]], seq[ids[1]])
	}
}

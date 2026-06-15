package head

import (
	"context"
	"path/filepath"
	"sync"
	"testing"
)

func testStore(t *testing.T) *Store {
	t.Helper()
	s, err := Open(filepath.Join(t.TempDir(), "head.db"))
	if err != nil {
		t.Fatalf("open: %v", err)
	}
	t.Cleanup(func() { _ = s.Close() })
	return s
}

func newAccount(t *testing.T, s *Store) string {
	t.Helper()
	a, err := s.CreateAccount(context.Background())
	if err != nil {
		t.Fatalf("create account: %v", err)
	}
	return a.ID
}

func balance(t *testing.T, s *Store, acct string) int64 {
	t.Helper()
	b, err := s.Balance(context.Background(), acct)
	if err != nil {
		t.Fatalf("balance: %v", err)
	}
	return b
}

// A credit and a debit move the materialized balance by exactly their signed
// amounts.
func TestLedgerCreditDebit(t *testing.T) {
	s := testStore(t)
	a := newAccount(t, s)
	ctx := context.Background()
	if err := s.Credit(ctx, a, 1_000_000, reasonFunding, "c1", nil); err != nil {
		t.Fatal(err)
	}
	if err := s.Debit(ctx, a, 250_000, reasonInference, "d1", nil); err != nil {
		t.Fatal(err)
	}
	if got := balance(t, s, a); got != 750_000 {
		t.Fatalf("balance = %d, want 750000", got)
	}
}

// A repeated ref is a no-op: neither a second entry nor a second balance move.
func TestLedgerIdempotentRef(t *testing.T) {
	s := testStore(t)
	a := newAccount(t, s)
	ctx := context.Background()
	for i := 0; i < 5; i++ {
		if err := s.Credit(ctx, a, 100_000, reasonFunding, "deposit:abc", nil); err != nil {
			t.Fatal(err)
		}
	}
	if got := balance(t, s, a); got != 100_000 {
		t.Fatalf("balance = %d, want 100000 (idempotent)", got)
	}
}

// Zero/negative amounts and unknown kinds are rejected before any balance move.
func TestLedgerRejectsBadAmount(t *testing.T) {
	s := testStore(t)
	a := newAccount(t, s)
	ctx := context.Background()
	if err := s.Post(ctx, a, kindCredit, 0, reasonFunding, "z", nil); err == nil {
		t.Fatal("expected error for zero amount")
	}
	if err := s.Post(ctx, a, kindCredit, -5, reasonFunding, "n", nil); err == nil {
		t.Fatal("expected error for negative amount")
	}
	if err := s.Post(ctx, a, "sideways", 1, reasonFunding, "k", nil); err == nil {
		t.Fatal("expected error for unknown kind")
	}
	if got := balance(t, s, a); got != 0 {
		t.Fatalf("balance moved on rejected posts: %d", got)
	}
}

// Concurrent distinct-ref credits all land: the materialized balance equals the
// exact sum, proving the write path doesn't lose updates under contention.
func TestLedgerConcurrentDistinctRefs(t *testing.T) {
	s := testStore(t)
	a := newAccount(t, s)
	const n = 50
	var wg sync.WaitGroup
	for i := 0; i < n; i++ {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			ref := "r" + string(rune('A'+i%26)) + string(rune('0'+i/26))
			if err := s.Credit(context.Background(), a, 1000, reasonFunding, ref, nil); err != nil {
				t.Errorf("credit %d: %v", i, err)
			}
		}(i)
	}
	wg.Wait()
	if got := balance(t, s, a); got != n*1000 {
		t.Fatalf("balance = %d, want %d", got, n*1000)
	}
}

// Concurrent same-ref credits collapse to exactly one: idempotency holds under a
// race, the canonical double-spend guard for redelivered deposits.
func TestLedgerConcurrentSameRef(t *testing.T) {
	s := testStore(t)
	a := newAccount(t, s)
	const n = 50
	var wg sync.WaitGroup
	for i := 0; i < n; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			_ = s.Credit(context.Background(), a, 1000, reasonFunding, "same-ref", nil)
		}()
	}
	wg.Wait()
	if got := balance(t, s, a); got != 1000 {
		t.Fatalf("balance = %d, want 1000 (one credit despite %d racers)", got, n)
	}
}

func TestMicroFromUSD(t *testing.T) {
	cases := []struct {
		usd  float64
		want int64
	}{
		{1, 1_000_000},
		{0.000001, 1},
		{0.0000004, 0}, // rounds down
		{0.0000006, 1}, // rounds up
		{3000, 3_000_000_000},
		{0.1, 100_000},
	}
	for _, c := range cases {
		if got := MicroFromUSD(c.usd); got != c.want {
			t.Errorf("MicroFromUSD(%v) = %d, want %d", c.usd, got, c.want)
		}
	}
}

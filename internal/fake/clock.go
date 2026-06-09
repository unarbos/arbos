package fake

import (
	"sync"
	"time"
)

// Clock is a fixed, monotonically-stepping clock for deterministic replay. It is
// safe for concurrent use: one Clock is shared across session actors, so Now is
// guarded (the ports.Clock contract requires concurrency safety).
type Clock struct {
	mu   sync.Mutex
	t    time.Time
	step time.Duration
}

func NewClock() *Clock {
	return &Clock{
		t:    time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC),
		step: time.Second,
	}
}

func (c *Clock) Now() time.Time {
	c.mu.Lock()
	defer c.mu.Unlock()
	now := c.t
	c.t = c.t.Add(c.step)
	return now
}

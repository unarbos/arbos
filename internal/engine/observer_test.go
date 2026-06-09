package engine_test

import (
	"context"
	"sync"
	"testing"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/fake"
)

// capturingObserver records the kinds of every observed event.
type capturingObserver struct {
	mu    sync.Mutex
	kinds []core.KernelEventKind
}

func (o *capturingObserver) ObserveEvent(ctx context.Context, ev core.KernelEvent) {
	o.mu.Lock()
	defer o.mu.Unlock()
	o.kinds = append(o.kinds, ev.Kind())
}

func (o *capturingObserver) saw(k core.KernelEventKind) bool {
	o.mu.Lock()
	defer o.mu.Unlock()
	for _, x := range o.kinds {
		if x == k {
			return true
		}
	}
	return false
}

func TestObserverSeesTurnEvents(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	obs := &capturingObserver{}
	eng := engine.New(fake.Provider{}, fake.Tools{}, fake.NewStore(), fake.NewClock(),
		engine.Config{Model: "fake", MaxIterations: 5}, engine.WithObserver(obs))
	conv, err := eng.StartSession(ctx, "s-obs")
	if err != nil {
		t.Fatal(err)
	}
	conv.Send(core.PromptIntent{Text: "please use the tool"})
	drain(t, conv)

	for _, want := range []core.KernelEventKind{core.KernelEventToolStarted, core.KernelEventToolFinished, core.KernelEventTurnComplete} {
		if !obs.saw(want) {
			t.Fatalf("observer never saw %q", want)
		}
	}
}

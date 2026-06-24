package settings

import (
	"path/filepath"
	"testing"
)

func openTmp(t *testing.T) *Store {
	t.Helper()
	s, err := Open(filepath.Join(t.TempDir(), "settings.json"))
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	return s
}

// reopen reloads the same path into a fresh store, proving the change reached
// disk (not just the in-memory cache).
func reopen(t *testing.T, s *Store) *Store {
	t.Helper()
	got, err := Open(s.path)
	if err != nil {
		t.Fatalf("reopen: %v", err)
	}
	return got
}

func TestAddProviderFirstBecomesActiveAndMirrors(t *testing.T) {
	s := openTmp(t)
	p := ProviderEntry{ID: "or", Label: "OpenRouter", ProviderName: "openai", Endpoint: "https://openrouter.ai/api/v1", KeyVaultName: "LLM_KEY_or", DefaultModel: "anthropic/claude-opus-4.8"}
	if err := s.AddProvider(p); err != nil {
		t.Fatalf("AddProvider: %v", err)
	}
	v := reopen(t, s).Get()
	if len(v.Providers) != 1 {
		t.Fatalf("want 1 provider, got %d", len(v.Providers))
	}
	if v.ActiveProviderID != "or" {
		t.Errorf("first add should become active, got %q", v.ActiveProviderID)
	}
	// Downgrade mirror points at the active entry.
	if v.LLMBaseURL != p.Endpoint || v.DefaultModel != p.DefaultModel {
		t.Errorf("mirror not set: base=%q model=%q", v.LLMBaseURL, v.DefaultModel)
	}
	act, ok := v.Active()
	if !ok || act.ID != "or" {
		t.Errorf("Active() = %+v, %v", act, ok)
	}
}

func TestSelectAmongMultipleUpdatesMirror(t *testing.T) {
	s := openTmp(t)
	must(t, s.AddProvider(ProviderEntry{ID: "or", ProviderName: "openai", Endpoint: "https://openrouter.ai/api/v1", DefaultModel: "m-or"}))
	must(t, s.AddProvider(ProviderEntry{ID: "gm", ProviderName: "openai", Endpoint: "https://api.saygm.com/v1", DefaultModel: "m-gm"}))
	// First add stays active; second add must not steal it.
	if got := s.Get().ActiveProviderID; got != "or" {
		t.Fatalf("active after second add = %q, want or", got)
	}
	must(t, s.SetActiveProvider("gm"))
	v := reopen(t, s).Get()
	if v.ActiveProviderID != "gm" {
		t.Fatalf("active = %q, want gm", v.ActiveProviderID)
	}
	if v.LLMBaseURL != "https://api.saygm.com/v1" || v.DefaultModel != "m-gm" {
		t.Errorf("mirror not following active: base=%q model=%q", v.LLMBaseURL, v.DefaultModel)
	}
	if err := s.SetActiveProvider("nope"); err == nil {
		t.Error("SetActiveProvider on unknown id should error")
	}
}

func TestUpdateProviderPreservesKeyAndOrder(t *testing.T) {
	s := openTmp(t)
	must(t, s.AddProvider(ProviderEntry{ID: "a", ProviderName: "openai", KeyVaultName: "LLM_KEY_a", Endpoint: "https://a"}))
	must(t, s.AddProvider(ProviderEntry{ID: "b", ProviderName: "google", KeyVaultName: "LLM_KEY_b", Endpoint: "https://b"}))
	// Edit "a" without supplying a key — the existing vault binding must survive.
	must(t, s.UpdateProvider(ProviderEntry{ID: "a", ProviderName: "openai", Endpoint: "https://a2", Label: "A2"}))
	v := reopen(t, s).Get()
	if len(v.Providers) != 2 || v.Providers[0].ID != "a" || v.Providers[1].ID != "b" {
		t.Fatalf("order not preserved: %+v", v.Providers)
	}
	if v.Providers[0].KeyVaultName != "LLM_KEY_a" {
		t.Errorf("key binding orphaned: %q", v.Providers[0].KeyVaultName)
	}
	if v.Providers[0].Endpoint != "https://a2" || v.Providers[0].Label != "A2" {
		t.Errorf("edit not applied: %+v", v.Providers[0])
	}
	if err := s.UpdateProvider(ProviderEntry{ID: "ghost"}); err == nil {
		t.Error("UpdateProvider on unknown id should error")
	}
}

func TestRemoveActiveReassignsAndMirrors(t *testing.T) {
	s := openTmp(t)
	must(t, s.AddProvider(ProviderEntry{ID: "a", ProviderName: "openai", Endpoint: "https://a", DefaultModel: "ma"}))
	must(t, s.AddProvider(ProviderEntry{ID: "b", ProviderName: "openai", Endpoint: "https://b", DefaultModel: "mb"}))
	must(t, s.SetActiveProvider("a"))
	must(t, s.RemoveProvider("a"))
	v := reopen(t, s).Get()
	if len(v.Providers) != 1 || v.Providers[0].ID != "b" {
		t.Fatalf("remove failed: %+v", v.Providers)
	}
	if v.ActiveProviderID != "b" {
		t.Errorf("active should reassign to b, got %q", v.ActiveProviderID)
	}
	if v.LLMBaseURL != "https://b" || v.DefaultModel != "mb" {
		t.Errorf("mirror not updated after remove: base=%q model=%q", v.LLMBaseURL, v.DefaultModel)
	}
	// Remove the last entry: no active, mirror left as-is (harmless legacy value).
	must(t, s.RemoveProvider("b"))
	v = reopen(t, s).Get()
	if len(v.Providers) != 0 || v.ActiveProviderID != "" {
		t.Errorf("expected empty providers and no active, got %+v / %q", v.Providers, v.ActiveProviderID)
	}
	if _, ok := v.Active(); ok {
		t.Error("Active() should be false with no providers")
	}
	if err := s.RemoveProvider("ghost"); err == nil {
		t.Error("RemoveProvider on unknown id should error")
	}
}

func TestActiveFallsBackToFirstOnStaleID(t *testing.T) {
	v := Settings{
		Providers:        []ProviderEntry{{ID: "x"}, {ID: "y"}},
		ActiveProviderID: "gone",
	}
	act, ok := v.Active()
	if !ok || act.ID != "x" {
		t.Errorf("stale active id should fall back to first, got %+v %v", act, ok)
	}
}

func must(t *testing.T, err error) {
	t.Helper()
	if err != nil {
		t.Fatal(err)
	}
}

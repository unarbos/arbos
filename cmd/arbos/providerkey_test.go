package main

import (
	"regexp"
	"testing"
)

// providerKeyName namespaces each provider's vault entry by id (ADR-0040), so
// multiple providers' credentials coexist under distinct names.
func TestProviderKeyName(t *testing.T) {
	if got := providerKeyName("ab12cd"); got != "LLM_KEY_ab12cd" {
		t.Errorf("providerKeyName = %q, want LLM_KEY_ab12cd", got)
	}
	// Distinct ids must yield distinct vault names (the whole point — no clash).
	if providerKeyName("a") == providerKeyName("b") {
		t.Error("distinct ids produced the same vault key name")
	}
}

// newProviderID mints a stable-shaped opaque id and never collides across a
// run of mints (12 hex chars from 6 random bytes).
func TestNewProviderID(t *testing.T) {
	re := regexp.MustCompile(`^[0-9a-f]{12}$`)
	seen := map[string]bool{}
	for i := 0; i < 1000; i++ {
		id := newProviderID()
		if !re.MatchString(id) {
			t.Fatalf("id %q does not match 12-hex shape", id)
		}
		if seen[id] {
			t.Fatalf("duplicate id minted: %q", id)
		}
		seen[id] = true
	}
}

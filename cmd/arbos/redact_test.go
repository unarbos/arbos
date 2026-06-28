package main

import "testing"

type fakeVault struct{ vals []string }

func (f fakeVault) EnvValues() []string { return f.vals }

// The outbound redaction twin scrubs managed-secret values from federated room
// prose, leaves non-secret text intact, and ignores short values (avoiding
// false positives on common words). A nil vault disables it.
func TestRedactSecrets(t *testing.T) {
	r := redactSecretsFn(fakeVault{vals: []string{
		"OPENAI_API_KEY=sk-supersecretlongtoken",
		"FLAG=true", // too short (<8) — must not scrub
	}})
	if r == nil {
		t.Fatal("redactor is nil with a vault present")
	}

	got := r("here is my key sk-supersecretlongtoken — keep it safe")
	if want := "here is my key [redacted secret] — keep it safe"; got != want {
		t.Errorf("secret not scrubbed:\n got %q\nwant %q", got, want)
	}

	// Non-secret prose is untouched, and a short value never triggers a scrub.
	if got := r("the flag is true and nothing leaks"); got != "the flag is true and nothing leaks" {
		t.Errorf("short value / plain text wrongly altered: %q", got)
	}

	if redactSecretsFn(nil) != nil {
		t.Error("redactor should be nil with no vault")
	}
}

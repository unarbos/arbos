package core_test

import (
	"encoding/json"
	"fmt"
	"testing"

	"github.com/unarbos/arbos/internal/core"
)

func TestSecretValueNeverLeaksViaStringOrJSON(t *testing.T) {
	const raw = "sk-supersecret-12345"
	v := core.NewSecretValue([]byte(raw))

	for name, got := range map[string]string{
		"String": v.String(),
		"%v":     fmt.Sprintf("%v", v),
		//nolint:staticcheck // deliberately exercising the %s verb, not String()
		"%s":  fmt.Sprintf("%s", v),
		"%#v": fmt.Sprintf("%#v", v),
	} {
		if got == raw {
			t.Fatalf("%s leaked the raw secret", name)
		}
		if got != "[REDACTED]" {
			t.Fatalf("%s = %q, want [REDACTED]", name, got)
		}
	}

	// As a struct field, too.
	out, err := json.Marshal(struct {
		Key core.SecretValue `json:"key"`
	}{Key: v})
	if err != nil {
		t.Fatal(err)
	}
	if string(out) != `{"key":"[REDACTED]"}` {
		t.Fatalf("json leaked or malformed: %s", out)
	}
}

func TestSecretValueExposeAndDestroy(t *testing.T) {
	v := core.NewSecretValue([]byte("abc"))
	if string(v.Expose()) != "abc" {
		t.Fatal("Expose should return the raw value at the boundary")
	}
	v.Destroy()
	for _, b := range v.Expose() {
		if b != 0 {
			t.Fatal("Destroy should zero the backing bytes")
		}
	}
}

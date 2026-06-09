package jsonschema_test

import (
	"encoding/json"
	"reflect"
	"testing"

	"github.com/unarbos/arbos/internal/tool/jsonschema"
)

type sample struct {
	Name    string   `json:"name" desc:"the name"`
	Count   int      `json:"count,omitempty"`
	Enabled bool     `json:"enabled"`
	Tags    []string `json:"tags,omitempty"`
	private string   //nolint:unused // exercises the unexported-skip path
	Skip    string   `json:"-"`
}

func TestReflectShapes(t *testing.T) {
	raw, err := jsonschema.Reflect(reflect.TypeOf(sample{}))
	if err != nil {
		t.Fatal(err)
	}
	var got map[string]any
	if err := json.Unmarshal(raw, &got); err != nil {
		t.Fatal(err)
	}

	if got["type"] != "object" {
		t.Fatalf("type: %v", got["type"])
	}
	if got["additionalProperties"] != false {
		t.Fatalf("additionalProperties should be false: %v", got["additionalProperties"])
	}

	props := got["properties"].(map[string]any)
	for _, name := range []string{"name", "count", "enabled", "tags"} {
		if _, ok := props[name]; !ok {
			t.Fatalf("missing property %q", name)
		}
	}
	if _, ok := props["private"]; ok {
		t.Fatal("unexported field must be skipped")
	}
	if _, ok := props["Skip"]; ok {
		t.Fatal(`json:"-" field must be skipped`)
	}

	// required = non-omitempty fields, in declaration order.
	req := toStrings(got["required"])
	want := []string{"name", "enabled"}
	if !reflect.DeepEqual(req, want) {
		t.Fatalf("required: got %v want %v", req, want)
	}

	if props["tags"].(map[string]any)["type"] != "array" {
		t.Fatalf("tags should be array: %v", props["tags"])
	}
	if props["name"].(map[string]any)["description"] != "the name" {
		t.Fatalf("description from desc tag missing: %v", props["name"])
	}
}

// TestDeterministic guarantees the generator produces byte-identical output for
// an unchanged type — the property the CI dirty-tree check relies on.
func TestDeterministic(t *testing.T) {
	a, _ := jsonschema.Reflect(reflect.TypeOf(sample{}))
	b, _ := jsonschema.Reflect(reflect.TypeOf(sample{}))
	if string(a) != string(b) {
		t.Fatalf("non-deterministic output:\n%s\n%s", a, b)
	}
}

func toStrings(v any) []string {
	if v == nil {
		return nil
	}
	arr := v.([]any)
	out := make([]string, len(arr))
	for i, x := range arr {
		out[i] = x.(string)
	}
	return out
}

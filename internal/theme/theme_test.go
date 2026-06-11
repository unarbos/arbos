package theme

import "testing"

func TestApplySwitchesPalettes(t *testing.T) {
	t.Cleanup(func() { Apply("dark") })

	Apply("light")
	if Text != Light.Text || Canvas != Light.Canvas || Primary != Light.Primary {
		t.Fatalf("light palette not applied: Text=%v Canvas=%v", Text, Canvas)
	}
	Apply("dark")
	if Text != Dark.Text || Canvas != Dark.Canvas {
		t.Fatalf("dark palette not applied: Text=%v Canvas=%v", Text, Canvas)
	}
	Apply("nonsense")
	if Text != Dark.Text {
		t.Fatalf("unknown name should keep dark default, got Text=%v", Text)
	}
}

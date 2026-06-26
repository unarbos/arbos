package tool

import (
	"os"
	"path/filepath"
	"testing"
)

// TestResolveDeniesKeyMaterial pins the ADR-0035/ADR-0041 denylist: the agent's
// own tools cannot resolve the device-key/homeserver-identity material, while
// the rest of the workspace — including .arbos/prompts and .arbos/skills —
// stays reachable (this is a secret denylist, not confinement).
func TestResolveDeniesKeyMaterial(t *testing.T) {
	base := "/work"
	denied := []string{
		".arbos/matrix/roomserver.db",
		".arbos/matrix",
		".arbos/identity/device.key",
		"sub/.arbos/matrix/key.pem", // anywhere in the tree
	}
	for _, rel := range denied {
		if _, err := Resolve(base, rel); err == nil {
			t.Errorf("Resolve(%q) should be denied (key material), but was allowed", rel)
		}
	}

	allowed := []string{
		"src/main.go",
		".arbos/prompts/poteto.md", // slash-command templates: agent-editable
		".arbos/skills/foo/SKILL.md",
		".arbos/sessions.db", // the brain is the user's own data, not a key
	}
	for _, rel := range allowed {
		if _, err := Resolve(base, rel); err != nil {
			t.Errorf("Resolve(%q) should be allowed, but was denied: %v", rel, err)
		}
	}
}

// TestResolveDeniesMachineConfigHome guards the most critical case: the
// machine-level ~/.config/arbos home (the device key + secret vault) is
// off-limits regardless of the working directory.
func TestResolveDeniesMachineConfigHome(t *testing.T) {
	cfg, err := os.UserConfigDir()
	if err != nil {
		t.Skipf("no user config dir on this platform: %v", err)
	}
	deviceKey := filepath.Join(cfg, "arbos", "identity", "device.key")
	if _, err := Resolve("/work", deviceKey); err == nil {
		t.Fatalf("Resolve(%q) should be denied (device key), but was allowed", deviceKey)
	}
}

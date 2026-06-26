package embed

import (
	"net/http"
	"testing"
	"time"
)

// TestEmbedServes proves the ADR-0041 single-binary path: an in-process Dendrite
// monolith, started purely through this public package (no internal/* imported
// by the caller), comes up and serves the Matrix client-server API on loopback.
func TestEmbedServes(t *testing.T) {
	srv, err := Start(Options{
		ServerName:       "localhost",
		DataDir:          t.TempDir(),
		BindAddr:         "127.0.0.1:18097",
		OpenRegistration: true,
	})
	if err != nil {
		t.Fatalf("Start: %v", err)
	}
	defer srv.Shutdown()

	url := srv.BaseURL + "/_matrix/client/versions"
	deadline := time.Now().Add(30 * time.Second)
	for time.Now().Before(deadline) {
		resp, err := http.Get(url)
		if err == nil {
			_ = resp.Body.Close()
			if resp.StatusCode == http.StatusOK {
				t.Logf("embedded Dendrite serving at %s", srv.BaseURL)
				return
			}
		}
		time.Sleep(300 * time.Millisecond)
	}
	t.Fatalf("embedded homeserver never served 200 at %s", url)
}

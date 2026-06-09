package mcp_test

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/mcp"
)

func TestConnectFakeServer(t *testing.T) {
	root, _ := os.Getwd()
	script := filepath.Join(root, "..", "..", "scripts", "fake-mcp-server.py")
	cfg := &mcp.Config{Servers: []mcp.ServerConfig{{
		Name: "fake", Command: "python3", Args: []string{script},
	}}}
	mgr, err := mcp.Connect(context.Background(), cfg)
	if err != nil {
		t.Fatal(err)
	}
	defer mgr.Close()
	if len(mgr.Runtimes()) != 1 {
		t.Fatalf("expected 1 runtime, got %d", len(mgr.Runtimes()))
	}
	schemas := mgr.Runtimes()[0].Schemas()
	if len(schemas) != 1 || schemas[0].Name != "fake__ping" {
		t.Fatalf("unexpected schemas: %+v", schemas)
	}
	res := mgr.Runtimes()[0].Dispatch(context.Background(), core.ToolCall{ID: "1", Name: "fake__ping", Args: json.RawMessage(`{}`)})
	if res.IsError || res.Content != "pong" {
		t.Fatalf("dispatch: isErr=%v content=%q", res.IsError, res.Content)
	}
}

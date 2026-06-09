package mcp

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	"github.com/unarbos/arbos/internal/ports"
)

// ServerConfig describes one MCP server subprocess to spawn.
type ServerConfig struct {
	Name    string   `json:"name"`
	Command string   `json:"command"`
	Args    []string `json:"args,omitempty"`
	Env     []string `json:"env,omitempty"`
}

// Config is the MCP configuration file shape.
type Config struct {
	Servers []ServerConfig `json:"servers"`
}

// Manager holds connected MCP server runtimes.
type Manager struct {
	runtimes []ports.ToolRuntime
	procs    []*exec.Cmd
}

// LoadConfig reads MCP config from, in order: ARBOS_MCP_CONFIG (JSON string),
// cwd/.arbos/mcp.json, userDir/mcp.json. Returns nil config when none found.
func LoadConfig(cwd, userDir string) (*Config, error) {
	if raw := strings.TrimSpace(os.Getenv("ARBOS_MCP_CONFIG")); raw != "" {
		var cfg Config
		if err := json.Unmarshal([]byte(raw), &cfg); err != nil {
			return nil, fmt.Errorf("ARBOS_MCP_CONFIG: %w", err)
		}
		return &cfg, nil
	}
	paths := []string{filepath.Join(cwd, ".arbos", "mcp.json")}
	if userDir != "" {
		paths = append(paths, filepath.Join(userDir, "mcp.json"))
	}
	for _, path := range paths {
		b, err := os.ReadFile(path)
		if err != nil {
			if os.IsNotExist(err) {
				continue
			}
			return nil, err
		}
		var cfg Config
		if err := json.Unmarshal(b, &cfg); err != nil {
			return nil, fmt.Errorf("parse %s: %w", path, err)
		}
		return &cfg, nil
	}
	return nil, nil
}

// Connect spawns configured MCP servers and returns a manager. cfg may be nil.
func Connect(ctx context.Context, cfg *Config) (*Manager, error) {
	if cfg == nil || len(cfg.Servers) == 0 {
		return &Manager{}, nil
	}
	m := &Manager{}
	for _, s := range cfg.Servers {
		if strings.TrimSpace(s.Name) == "" || strings.TrimSpace(s.Command) == "" {
			return nil, fmt.Errorf("mcp server requires name and command")
		}
		cmd := exec.CommandContext(ctx, s.Command, s.Args...)
		if len(s.Env) > 0 {
			cmd.Env = append(os.Environ(), s.Env...)
		}
		stdin, err := cmd.StdinPipe()
		if err != nil {
			m.Close()
			return nil, err
		}
		stdout, err := cmd.StdoutPipe()
		if err != nil {
			m.Close()
			return nil, err
		}
		if err := cmd.Start(); err != nil {
			m.Close()
			return nil, fmt.Errorf("mcp start %q: %w", s.Name, err)
		}
		client := NewClient(stdout, stdin)
		if err := client.Initialize(ctx); err != nil {
			m.Close()
			return nil, fmt.Errorf("mcp initialize %q: %w", s.Name, err)
		}
		rt, err := client.Runtime(ctx, s.Name)
		if err != nil {
			m.Close()
			return nil, fmt.Errorf("mcp runtime %q: %w", s.Name, err)
		}
		m.runtimes = append(m.runtimes, rt)
		m.procs = append(m.procs, cmd)
	}
	return m, nil
}

// Runtimes returns the MCP tool runtimes for merging into the host toolset.
func (m *Manager) Runtimes() []ports.ToolRuntime { return m.runtimes }

// Close stops all MCP server subprocesses.
func (m *Manager) Close() {
	for _, cmd := range m.procs {
		if cmd.Process != nil {
			_ = cmd.Process.Kill()
		}
		_ = cmd.Wait()
	}
	m.procs = nil
	m.runtimes = nil
}

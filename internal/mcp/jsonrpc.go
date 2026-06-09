// Package mcp is a minimal Model Context Protocol implementation over stdio
// JSON-RPC 2.0: a client that maps an external MCP server's tools into an arbos
// ports.ToolRuntime, and a server that exposes an arbos ToolRuntime to external
// MCP runtimes. Messages are newline-delimited JSON (the MCP stdio framing).
//
// Faithful to the MCP tool surface pi consumes/exposes (initialize, tools/list,
// tools/call). Resources and prompts are out of scope for the coding tool path;
// the seam is here if they are added.
//
// Staged: client and server are built and tested (ADR-0029) but no entrypoint
// spawns an MCP server or exposes arbos's tools over MCP yet, so this is
// experimental until a host wires it in.
package mcp

import "encoding/json"

const protocolVersion = "2024-11-05"

type rpcRequest struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      *int            `json:"id,omitempty"` // omitted for notifications
	Method  string          `json:"method"`
	Params  json.RawMessage `json:"params,omitempty"`
}

type rpcResponse struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      *int            `json:"id,omitempty"`
	Result  json.RawMessage `json:"result,omitempty"`
	Error   *rpcError       `json:"error,omitempty"`
}

type rpcError struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
}

// Tool is an MCP tool descriptor (tools/list entry).
type Tool struct {
	Name        string          `json:"name"`
	Description string          `json:"description,omitempty"`
	InputSchema json.RawMessage `json:"inputSchema,omitempty"`
}

type toolsListResult struct {
	Tools []Tool `json:"tools"`
}

type toolsCallParams struct {
	Name      string          `json:"name"`
	Arguments json.RawMessage `json:"arguments,omitempty"`
}

type contentBlock struct {
	Type string `json:"type"`
	Text string `json:"text,omitempty"`
}

type toolsCallResult struct {
	Content []contentBlock `json:"content"`
	IsError bool           `json:"isError,omitempty"`
}

type initializeResult struct {
	ProtocolVersion string          `json:"protocolVersion"`
	Capabilities    json.RawMessage `json:"capabilities,omitempty"`
	ServerInfo      serverInfo      `json:"serverInfo"`
}

type serverInfo struct {
	Name    string `json:"name"`
	Version string `json:"version"`
}

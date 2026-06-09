package mcp

import (
	"bufio"
	"context"
	"encoding/json"
	"io"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/ports"
)

// Serve exposes an arbos ToolRuntime over MCP JSON-RPC, so an external runtime
// (e.g. another agent) can use arbos's tools. It handles initialize, tools/list,
// and tools/call, reading newline-delimited requests from r and writing
// responses to w until EOF or ctx cancellation.
func Serve(ctx context.Context, rt ports.ToolRuntime, r io.Reader, w io.Writer) error {
	sc := bufio.NewScanner(r)
	sc.Buffer(make([]byte, 0, 64*1024), 4*1024*1024)
	write := func(v any) error {
		b, err := json.Marshal(v)
		if err != nil {
			return err
		}
		_, err = w.Write(append(b, '\n'))
		return err
	}

	for sc.Scan() {
		if ctx.Err() != nil {
			return ctx.Err()
		}
		var req rpcRequest
		if json.Unmarshal(sc.Bytes(), &req) != nil {
			continue
		}
		switch req.Method {
		case "initialize":
			if err := write(rpcResponse{JSONRPC: "2.0", ID: req.ID, Result: mustJSON(initializeResult{
				ProtocolVersion: protocolVersion,
				Capabilities:    json.RawMessage(`{"tools":{}}`),
				ServerInfo:      serverInfo{Name: "arbos", Version: "0"},
			})}); err != nil {
				return err
			}
		case "notifications/initialized":
			// notification, no response
		case "tools/list":
			var tools []Tool
			for _, s := range rt.Schemas() {
				params := s.Parameters
				if len(params) == 0 {
					params = json.RawMessage(`{"type":"object"}`)
				}
				tools = append(tools, Tool{Name: s.Name, Description: s.Description, InputSchema: params})
			}
			if err := write(rpcResponse{JSONRPC: "2.0", ID: req.ID, Result: mustJSON(toolsListResult{Tools: tools})}); err != nil {
				return err
			}
		case "tools/call":
			var p toolsCallParams
			_ = json.Unmarshal(req.Params, &p)
			res := rt.Dispatch(ctx, core.ToolCall{ID: "mcp", Name: p.Name, Args: p.Arguments})
			if err := write(rpcResponse{JSONRPC: "2.0", ID: req.ID, Result: mustJSON(toolsCallResult{
				Content: []contentBlock{{Type: "text", Text: res.Content}},
				IsError: res.IsError,
			})}); err != nil {
				return err
			}
		default:
			if req.ID != nil {
				if err := write(rpcResponse{JSONRPC: "2.0", ID: req.ID, Error: &rpcError{Code: -32601, Message: "method not found: " + req.Method}}); err != nil {
					return err
				}
			}
		}
	}
	return sc.Err()
}

func mustJSON(v any) json.RawMessage {
	b, _ := json.Marshal(v)
	return b
}

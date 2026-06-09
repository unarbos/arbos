package mcp

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"strings"
	"sync"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/ports"
)

// callTimeout bounds a single request/response when the caller's context has no
// deadline of its own, so a hung MCP server cannot wedge a session forever.
const callTimeout = 60 * time.Second

// Client speaks MCP JSON-RPC to a server over r/w (typically a spawned MCP
// server subprocess's stdout/stdin). A single reader goroutine demultiplexes
// responses to per-request waiters by id, so a request whose context is
// cancelled abandons only its own waiter — there is never a second goroutine
// racing on the reader, and a hung server times out instead of blocking forever.
type Client struct {
	w io.Writer

	wmu sync.Mutex // serializes writes; held only around the Write, never with mu

	mu      sync.Mutex // guards id, pending, closed
	id      int
	pending map[int]chan rpcResponse
	closed  bool
}

// NewClient builds a client over an MCP server's output (r) and input (w) and
// starts the reader goroutine. The goroutine exits when r reaches EOF.
func NewClient(r io.Reader, w io.Writer) *Client {
	c := &Client{w: w, pending: make(map[int]chan rpcResponse)}
	go c.readLoop(r)
	return c
}

func (c *Client) readLoop(r io.Reader) {
	sc := bufio.NewScanner(r)
	sc.Buffer(make([]byte, 0, 64*1024), 4*1024*1024)
	for sc.Scan() {
		var resp rpcResponse
		if json.Unmarshal(sc.Bytes(), &resp) != nil {
			continue
		}
		if resp.ID == nil {
			continue // a notification, not a response to a call
		}
		c.mu.Lock()
		ch := c.pending[*resp.ID]
		delete(c.pending, *resp.ID)
		c.mu.Unlock()
		if ch != nil {
			ch <- resp
		}
	}
	// Stream ended: close every outstanding waiter so blocked calls return.
	c.mu.Lock()
	c.closed = true
	for id, ch := range c.pending {
		close(ch)
		delete(c.pending, id)
	}
	c.mu.Unlock()
}

// writeMsg serializes outbound frames under wmu only — never under mu — so a
// server that stops draining stdin (a blocked Write) cannot wedge readLoop,
// which needs mu to deliver responses.
func (c *Client) writeMsg(v any) error {
	b, err := json.Marshal(v)
	if err != nil {
		return err
	}
	c.wmu.Lock()
	defer c.wmu.Unlock()
	_, err = c.w.Write(append(b, '\n'))
	return err
}

func (c *Client) call(ctx context.Context, method string, params any) (json.RawMessage, error) {
	if _, ok := ctx.Deadline(); !ok {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(ctx, callTimeout)
		defer cancel()
	}
	var praw json.RawMessage
	if params != nil {
		b, err := json.Marshal(params)
		if err != nil {
			return nil, err
		}
		praw = b
	}

	c.mu.Lock()
	if c.closed {
		c.mu.Unlock()
		return nil, io.EOF
	}
	c.id++
	id := c.id
	ch := make(chan rpcResponse, 1)
	c.pending[id] = ch
	c.mu.Unlock()

	// Write outside mu so a blocking Write can't starve readLoop.
	if err := c.writeMsg(rpcRequest{JSONRPC: "2.0", ID: &id, Method: method, Params: praw}); err != nil {
		c.mu.Lock()
		delete(c.pending, id)
		c.mu.Unlock()
		return nil, err
	}

	select {
	case <-ctx.Done():
		c.mu.Lock()
		delete(c.pending, id)
		c.mu.Unlock()
		return nil, fmt.Errorf("mcp %s: %w", method, ctx.Err())
	case resp, ok := <-ch:
		if !ok {
			return nil, fmt.Errorf("mcp %s: connection closed", method)
		}
		if resp.Error != nil {
			return nil, fmt.Errorf("mcp %s: %s", method, resp.Error.Message)
		}
		return resp.Result, nil
	}
}

func (c *Client) notify(method string) error {
	// writeMsg serializes under wmu; no mu needed (no pending entry to track).
	return c.writeMsg(rpcRequest{JSONRPC: "2.0", Method: method})
}

// Initialize performs the MCP handshake.
func (c *Client) Initialize(ctx context.Context) error {
	_, err := c.call(ctx, "initialize", map[string]any{
		"protocolVersion": protocolVersion,
		"capabilities":    map[string]any{},
		"clientInfo":      map[string]any{"name": "arbos", "version": "0"},
	})
	if err != nil {
		return err
	}
	return c.notify("notifications/initialized")
}

// ListTools returns the server's advertised tools.
func (c *Client) ListTools(ctx context.Context) ([]Tool, error) {
	raw, err := c.call(ctx, "tools/list", map[string]any{})
	if err != nil {
		return nil, err
	}
	var res toolsListResult
	if err := json.Unmarshal(raw, &res); err != nil {
		return nil, err
	}
	return res.Tools, nil
}

// CallTool invokes a tool and returns its joined text content and error flag.
func (c *Client) CallTool(ctx context.Context, name string, args json.RawMessage) (string, bool, error) {
	raw, err := c.call(ctx, "tools/call", toolsCallParams{Name: name, Arguments: args})
	if err != nil {
		return "", false, err
	}
	var res toolsCallResult
	if err := json.Unmarshal(raw, &res); err != nil {
		return "", false, err
	}
	var b strings.Builder
	for _, blk := range res.Content {
		if blk.Type == "text" {
			b.WriteString(blk.Text)
		}
	}
	return b.String(), res.IsError, nil
}

// Runtime exposes the server's tools as a ports.ToolRuntime, prefixing names
// with prefix+"__" to avoid colliding with built-in tools. It lists tools once.
func (c *Client) Runtime(ctx context.Context, prefix string) (ports.ToolRuntime, error) {
	tools, err := c.ListTools(ctx)
	if err != nil {
		return nil, err
	}
	schemas := make([]core.ToolSchema, 0, len(tools))
	names := make(map[string]string, len(tools))
	for _, t := range tools {
		pn := prefix + "__" + t.Name
		params := t.InputSchema
		if len(params) == 0 {
			params = json.RawMessage(`{"type":"object"}`)
		}
		schemas = append(schemas, core.ToolSchema{Name: pn, Description: t.Description, Parameters: params})
		names[pn] = t.Name
	}
	return &runtime{client: c, schemas: schemas, names: names}, nil
}

type runtime struct {
	client  *Client
	schemas []core.ToolSchema
	names   map[string]string
}

func (m *runtime) Schemas() []core.ToolSchema { return m.schemas }

func (m *runtime) Dispatch(ctx context.Context, call core.ToolCall) core.ToolResult {
	orig, ok := m.names[call.Name]
	if !ok {
		return core.ToolResult{CallID: call.ID, IsError: true, Content: "unknown mcp tool: " + call.Name}
	}
	text, isErr, err := m.client.CallTool(ctx, orig, call.Args)
	if err != nil {
		return core.ToolResult{CallID: call.ID, IsError: true, Content: err.Error()}
	}
	return core.ToolResult{CallID: call.ID, Content: text, IsError: isErr}
}

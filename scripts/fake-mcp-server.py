#!/usr/bin/env python3
"""Minimal MCP stdio server for arbos e2e tests."""
import json
import sys

def reply(rid, result):
    print(json.dumps({"jsonrpc": "2.0", "id": rid, "result": result}), flush=True)

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    rid = req.get("id")
    method = req.get("method")
    if method == "initialize":
        reply(rid, {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "serverInfo": {"name": "fake-mcp", "version": "0"},
        })
        print(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}), flush=True)
    elif method == "tools/list":
        reply(rid, {"tools": [{
            "name": "ping",
            "description": "Return pong",
            "inputSchema": {"type": "object", "properties": {}},
        }]})
    elif method == "tools/call":
        reply(rid, {"content": [{"type": "text", "text": "pong"}], "isError": False})

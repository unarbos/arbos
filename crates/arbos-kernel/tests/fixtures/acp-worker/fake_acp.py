#!/usr/bin/env python3
"""A tiny Agent Client Protocol agent for the fixture: JSON-RPC over stdio.

On a prompt it thinks, reads hello.txt through the client (fs/read_text_file),
reports the read as a tool call, asks permission to write, writes out.txt
through the client (fs/write_text_file), streams a reply, and ends the turn.
"""
import json, sys

def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n"); sys.stdout.flush()

def request(id_, method, params):
    send({"jsonrpc": "2.0", "id": id_, "method": method, "params": params})
    while True:
        line = sys.stdin.readline()
        if not line:
            sys.exit(0)
        msg = json.loads(line)
        if msg.get("id") == id_ and "method" not in msg:
            return msg

def notify(method, params):
    send({"jsonrpc": "2.0", "method": method, "params": params})

for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method"); mid = msg.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": mid, "result": {"protocolVersion": 1, "agentCapabilities": {"loadSession": False}, "agentInfo": {"name": "fake-acp", "version": "0"}}})
    elif method == "session/new":
        send({"jsonrpc": "2.0", "id": mid, "result": {"sessionId": "s1"}})
    elif method == "session/prompt":
        sid = msg["params"]["sessionId"]
        prompt = msg["params"]["prompt"][0]["text"]
        notify("session/update", {"sessionId": sid, "update": {"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "Let me read hello.txt."}}})
        got = request(100, "fs/read_text_file", {"sessionId": sid, "path": "hello.txt"})
        content = got.get("result", {}).get("content", "")
        notify("session/update", {"sessionId": sid, "update": {"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Read hello.txt", "kind": "read", "status": "in_progress", "rawInput": {"path": "hello.txt"}}})
        notify("session/update", {"sessionId": sid, "update": {"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed", "content": [{"type": "content", "content": {"type": "text", "text": content}}], "locations": [{"path": "hello.txt"}]}})
        perm = request(101, "session/request_permission", {"sessionId": sid, "toolCall": {"toolCallId": "t2", "title": "Write out.txt", "kind": "edit"}, "options": [{"optionId": "y", "kind": "allow_once", "name": "Allow"}, {"optionId": "n", "kind": "reject_once", "name": "Reject"}]})
        chosen = perm.get("result", {}).get("outcome", {}).get("optionId")
        if chosen == "y":
            request(102, "fs/write_text_file", {"sessionId": sid, "path": "out.txt", "content": "done by acp: " + content.strip() + "\n"})
            notify("session/update", {"sessionId": sid, "update": {"sessionUpdate": "tool_call_update", "toolCallId": "t2", "status": "completed", "content": [{"type": "content", "content": {"type": "text", "text": "wrote out.txt"}}]}})
            reply = "Wrote out.txt with: " + content.strip()
        else:
            reply = "Write was refused; nothing changed."
        for piece in reply.split(" "):
            notify("session/update", {"sessionId": sid, "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": piece + " "}}})
        send({"jsonrpc": "2.0", "id": mid, "result": {"stopReason": "end_turn"}})
    elif method == "session/cancel":
        pass
    elif mid is not None:
        send({"jsonrpc": "2.0", "id": mid, "error": {"code": -32601, "message": "method not found: " + str(method)}})

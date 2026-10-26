// Route guard vs. steers (#1690, #1704), driven through a fake `pi` API: `node --test`.
// Run with ARBOS_HOOK_URL unset so no agentd hook is called; the audit file goes to a temp dir.
import assert from "node:assert/strict";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

process.env.ARBOS_STATE_DIR = mkdtempSync(join(tmpdir(), "arbos-pi-ext-test-"));
process.env.ARBOS_HOOK_URL = "";
process.env.ARBOS_SESSION_ROLE = "main";

const { default: arbosPiExt, promisesMore, detachedShell } = await import("./index.ts");

type Handler = (event: any) => Promise<any> | any;

/** The slice of pi's ExtensionAPI the extension touches at registration and in these paths. */
function fakePi() {
  const handlers = new Map<string, Handler[]>();
  const tools = new Map<string, any>();
  const pi = {
    on(event: string, handler: Handler) {
      handlers.set(event, [...(handlers.get(event) ?? []), handler]);
      return () => {};
    },
    registerTool(tool: any) {
      tools.set(tool.name, tool);
    },
    appendEntry() {},
  };
  arbosPiExt(pi as any);
  async function emit(event: string, payload: Record<string, unknown>) {
    let result: any;
    for (const h of handlers.get(event) ?? []) result = await h({ type: event, ...payload });
    return result;
  }
  return { emit, tools };
}

const STEER = "[Arbos steer: the user sent this at 03:31:05 UTC, 28 s into your `bash` call. Arbos stopped that call so this could reach you at once; its process is gone, nothing of it still runs. It is not late. Act on it now: resume the interrupted work yourself from where its output (below) stops, with the change applied, and finish it; ask only if the change itself is ambiguous. When you mention timing, use the time above. This is the same turn, continued: do not call `route` again.]\nChange of plan: stop at 20, not 40.";

const bashCall = { toolCallId: "call-2", toolName: "bash", input: { command: "cat count.txt" }, parentToolCallId: null };

test("a plain user prompt gets the route reminder and its first tool call must be route", async () => {
  const { emit } = fakePi();
  const input = await emit("input", { text: "Count to 40.", source: "rpc" });
  assert.equal(input.action, "transform");
  assert.ok(input.text.startsWith("(Arbos: call `route` first.) "), input.text);
  await emit("before_agent_start", { messageType: "user", prompt: input.text, systemPromptOptions: { sections: {} } });
  const verdict = await emit("tool_call", bashCall);
  assert.equal(verdict?.block, true);
  assert.match(verdict.reason, /Call `route` first/);
});

test("a stamped steer gets no route reminder (#1690)", async () => {
  const { emit } = fakePi();
  const input = await emit("input", { text: STEER, source: "rpc", streamingBehavior: "steer" });
  assert.deepEqual(input, { action: "continue" });
});

test("the continuation after an interrupting steer is not re-gated: bash runs without a second route (#1704)", async () => {
  const { emit } = fakePi();
  // The turn that was interrupted: routed, then aborted by agentd.
  await emit("before_agent_start", { messageType: "user", prompt: "(Arbos: call `route` first.) Count to 40.", systemPromptOptions: { sections: {} } });
  await emit("agent_settled", { aborted: true });
  // agentd continues the same turn with the stamped steer as the prompt (`drive_turn`).
  const input = await emit("input", { text: STEER, source: "rpc" });
  assert.deepEqual(input, { action: "continue" });
  await emit("before_agent_start", { messageType: "user", prompt: STEER, systemPromptOptions: { sections: {} } });
  const verdict = await emit("tool_call", bashCall);
  assert.equal(verdict, undefined, `bash was blocked: ${JSON.stringify(verdict)}`);
});

test("the next ordinary turn after a steered continuation is gated again", async () => {
  const { emit } = fakePi();
  await emit("before_agent_start", { messageType: "user", prompt: STEER, systemPromptOptions: { sections: {} } });
  assert.equal(await emit("tool_call", bashCall), undefined);
  await emit("agent_settled", { aborted: false });
  await emit("before_agent_start", { messageType: "user", prompt: "(Arbos: call `route` first.) Done?", systemPromptOptions: { sections: {} } });
  const verdict = await emit("tool_call", bashCall);
  assert.equal(verdict?.block, true);
});

test("a relay turn: send_message before route is the reply route, not a blocked call (#1818)", async () => {
  const { emit } = fakePi();
  const input = await emit("input", { text: "[Arbos relay turn]\n[Arbos] Job \"Searching\" (j1) finished · status done · 100 s.", source: "rpc" });
  assert.equal(input.action, "transform");
  await emit("before_agent_start", { messageType: "user", prompt: input.text, systemPromptOptions: { sections: {} } });
  const verdict = await emit("tool_call", { toolCallId: "c1", toolName: "send_message", input: { text: "Here it is." }, parentToolCallId: null });
  assert.equal(verdict, undefined, `send_message was blocked: ${JSON.stringify(verdict)}`);
});

test("a reply text that promises parts does not end the turn; a complete reply does", async () => {
  const { emit, tools } = fakePi();
  const input = await emit("input", { text: "[Arbos relay turn]\n[Arbos] Job \"Searching\" (j1) finished · status done · 100 s.", source: "rpc" });
  await emit("before_agent_start", { messageType: "user", prompt: input.text, systemPromptOptions: { sections: {} } });
  const route = tools.get("route");
  const r1 = await route.execute("c1", { action: "reply", reply: "Here are seven well-rated San Jose restaurants, delivered in parts below." });
  assert.notEqual(r1.terminate, true);
  assert.match(r1.content[0].text, /post the rest with send_message/);
  // A user turn with a complete one-line answer still ends on the route call.
  const { emit: emit2, tools: tools2 } = fakePi();
  const in2 = await emit2("input", { text: "What is 6 × 7?", source: "rpc" });
  await emit2("before_agent_start", { messageType: "user", prompt: in2.text, systemPromptOptions: { sections: {} } });
  const r2 = await tools2.get("route").execute("c2", { action: "reply", reply: "42." });
  assert.equal(r2.terminate, true);
  const r3 = await tools2.get("route").execute("c3", { action: "reply", reply: "Seven places made the cut; the table follows." });
  assert.notEqual(r3.terminate, true);
  assert.equal(promisesMore("London is 7 hours ahead of you right now."), false);
  assert.equal(promisesMore("Details and sources below."), true);
  assert.equal(promisesMore("Here are the three schools, in parts."), true);
  assert.equal(promisesMore("The restaurant on the corner below the hotel is good, open till 10, no booking needed; the menu changes weekly and the staff speak English, so you will be fine there tonight."), false);
});

test("a detached script in the main turn is refused with the start_job hint (#1778, b9)", async () => {
  assert.ok(detachedShell("bash", { command: "nohup bash job.sh > job.log 2>&1 &" }));
  assert.ok(detachedShell("bash", { command: "cat > job.sh <<'EOF'\nsleep 45\nEOF\nbash job.sh &\necho started" }));
  assert.ok(detachedShell("bash", { command: "(sleep 45; echo done) & disown" }));
  assert.ok(detachedShell("bash", { command: "setsid python3 worker.py" }));
  assert.ok(detachedShell("bash", { command: "tmux new -d -s job 'sleep 45'" }));
  assert.ok(detachedShell("codemode", { code: "await tools.bash({command: 'nohup ./run.sh &'}); return 1" }));
  assert.equal(detachedShell("bash", { command: "ls -la && cat a.txt" }), null);
  assert.equal(detachedShell("bash", { command: "cargo test 2>&1 | tail -20" }), null);
  assert.equal(detachedShell("bash", { command: "echo 'a & b' > f.txt; grep -c '&' f.txt" }), null);
  assert.equal(detachedShell("bash", { command: "curl -s 'https://x.test/a?b=1&c=2'" }), null);
  assert.equal(detachedShell("read", { path: "/x" }), null);
  const { emit, tools } = fakePi();
  const input = await emit("input", { text: "Start a job with two steps, as a job (not in this turn).", source: "rpc" });
  await emit("before_agent_start", { messageType: "user", prompt: input.text, systemPromptOptions: { sections: {} } });
  await tools.get("route").execute("r", { action: "act", ack: "Starting it." });
  const verdict = await emit("tool_call", { toolCallId: "b", toolName: "bash", input: { command: "nohup bash job.sh &" }, parentToolCallId: null });
  assert.equal(verdict?.block, true);
  assert.match(verdict.reason, /start_job/);
  const fg = await emit("tool_call", { toolCallId: "b2", toolName: "bash", input: { command: "seq 1 10 > step1.txt" }, parentToolCallId: null });
  assert.equal(fg, undefined);
});

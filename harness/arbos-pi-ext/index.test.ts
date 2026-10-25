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

const { default: arbosPiExt } = await import("./index.ts");

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

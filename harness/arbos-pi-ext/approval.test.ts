import assert from "node:assert/strict";
import { createServer } from "node:http";
import { copyFileSync, mkdtempSync, readFileSync, readdirSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, test } from "node:test";
import { isDeepStrictEqual } from "node:util";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as Arbitrary from "effect/Arbitrary";
import * as Effect from "effect/Effect";
import * as Schema from "effect/Schema";
import { approvalTransition } from "./approval.ts";

type Reply = { body: unknown; raw?: string; status?: number; disconnect?: boolean };
let initial: Reply = { body: { ok: true, state: "allowed" } };
let statuses: Reply[] = [];
let parked: Reply = { body: { ok: true, state: "pending", parked: true, reason: "Waiting for the owner's decision." } };
let requests: string[] = [];

const server = createServer((req, res) => {
  req.resume();
  req.on("end", () => {
    assert.equal(req.headers.authorization, "Bearer synthetic-approval-token");
    requests.push(req.url ?? "");
    const reply = req.url === "/hook/approval" ? initial
      : req.url === "/hook/approval-status" ? (statuses.shift() ?? { body: { ok: true, state: "pending" } })
      : req.url === "/hook/approval-park" ? parked
      : undefined;
    assert.ok(reply, `unexpected hook ${req.url}`);
    if (reply.disconnect) { req.socket.destroy(); return; }
    res.writeHead(reply.status ?? 200, { "content-type": "application/json" });
    res.end(reply.raw ?? JSON.stringify(reply.body));
  });
});
await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
after(() => new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve())));

const port = (server.address() as { port: number }).port;
const stateDir = mkdtempSync(join(tmpdir(), "arbos-approval-test-"));
process.env.ARBOS_STATE_DIR = stateDir;
process.env.ARBOS_AUDIT_PATH = join(stateDir, "audit.jsonl");
process.env.ARBOS_BLOB_DIR = join(stateDir, "blobs");
process.env.ARBOS_HOOK_URL = `http://127.0.0.1:${port}`;
process.env.ARBOS_MCP_TOKEN = "synthetic-approval-token";
process.env.ARBOS_SESSION_ROLE = "subagent";
process.env.ARBOS_APPROVAL_POLL_MS = "1";
process.env.ARBOS_APPROVAL_WAIT_MS = "20";

const { default: extension } = await import("./index.ts");

function toolCall(register = extension, command = "printf approval-test"): () => Promise<any> {
  let handler: ((event: any) => Promise<any>) | undefined;
  register({
    on: (name: string, callback: any) => { if (name === "tool_call") handler = callback; },
    registerTool: () => {},
    appendEntry: () => {},
  } as any);
  assert.ok(handler);
  return () => handler!({ toolName: "bash", toolCallId: "approval-call", input: { command }, parentToolCallId: null });
}

function scenario(first: Reply, polls: Reply[] = [], park?: Reply): void {
  initial = first;
  statuses = polls;
  parked = park ?? { body: { ok: true, state: "pending", parked: true, reason: "Waiting for the owner's decision." } };
  requests = [];
}

const pending = (waitMs = 1000) => ({ body: { ok: true, state: "pending", approvalId: "approval-1", title: "Synthetic action", why: "test", waitMs, park: true, chat: "Owner", chatId: "chat-1" } });

test("a policy-allowed shell call continues without polling", async () => {
  scenario({ body: { ok: true, state: "allowed" } });
  assert.equal(await toolCall()(), undefined);
  assert.deepEqual(requests, ["/hook/approval"]);
});

test("an existing single-use grant still allows the call", async () => {
  scenario({ body: { ok: true, state: "allowed", approvalId: "grant-1" } });
  assert.equal(await toolCall()(), undefined);
  scenario({ body: { ok: true, state: "allowed", grantId: "standing-grant-1" } });
  assert.equal(await toolCall()(), undefined);
});

test("a pending card approved during grace admits the original call", async () => {
  scenario(pending(), [{ body: { ok: true, state: "approved", reason: null, decidedBy: "owner" } }]);
  assert.equal(await toolCall()(), undefined);
  assert.ok(requests.includes("/hook/approval-status"));
  assert.ok(!requests.includes("/hook/approval-park"));
  const events = readFileSync(join(stateDir, "audit.jsonl"), "utf8").trim().split("\n").map((line) => JSON.parse(line));
  assert.ok(events.some((event) => event.kind === "approval_result" && event.state === "approved" && event.approval_id === "approval-1"));
});

test("a rejected card blocks and carries the owner's explanation", async () => {
  scenario(pending(), [{ body: { ok: true, state: "rejected", reason: "Please keep this file.", decidedBy: "owner" } }]);
  const result = await toolCall()();
  assert.equal(result?.block, true);
  assert.match(result.reason, /user rejected/);
  assert.match(result.reason, /Please keep this file/);
});

test("a cancelled card blocks without calling it a rejection", async () => {
  scenario(pending(), [{ body: { ok: true, state: "cancelled", reason: null, decidedBy: null } }]);
  const result = await toolCall()();
  assert.equal(result?.block, true);
  assert.match(result.reason, /work was cancelled/);
  assert.doesNotMatch(result.reason, /user rejected/);
});

test("a legacy expired card never admits execution", async () => {
  scenario(pending(), [{ body: { ok: true, state: "expired", reason: null, decidedBy: null } }]);
  const result = await toolCall()();
  assert.equal(result?.block, true);
  assert.match(result.reason, /Nothing ran/);
});

test("parking a pending card blocks the client while the card remains answerable", async () => {
  scenario(pending(1));
  const result = await toolCall()();
  assert.equal(result?.block, true);
  assert.equal(result.reason, "Waiting for the owner's decision.");
  assert.ok(requests.includes("/hook/approval-park"));
});

test("approval winning the park race still admits the original call once", async () => {
  scenario(pending(1), [], { body: { ok: true, state: "approved", parked: false } });
  assert.equal(await toolCall()(), undefined);
  assert.ok(requests.includes("/hook/approval-park"));
});

test("a still-pending card that could not be parked does not admit execution", async () => {
  scenario(pending(1), [], { body: { ok: true, state: "pending", parked: false } });
  const result = await toolCall()();
  assert.equal(result?.block, true);
  assert.match(result.reason, /Nothing ran/);
});

test("an explicitly unconfigured local run keeps standalone shell behavior", async () => {
  const configured = process.env.ARBOS_HOOK_URL;
  delete process.env.ARBOS_HOOK_URL;
  let standalone;
  try {
    standalone = (await import(new URL("./index.ts?standalone-approval-test", import.meta.url).href)).default;
  } finally {
    process.env.ARBOS_HOOK_URL = configured;
  }
  scenario({ body: { ok: true, state: "allowed" } });
  assert.equal(await toolCall(standalone)(), undefined);
  assert.deepEqual(requests, []);
});

test("a configured approval hook failure blocks instead of running the shell call", async () => {
  const privateDetail = "SYNTHETIC_PRIVATE_APPROVAL_DETAIL";
  scenario({ body: { ok: false, error: privateDetail } });
  const result = await toolCall()();
  assert.equal(result?.block, true, "a configured approval failure must not grant execution");
  assert.doesNotMatch(result.reason, /SYNTHETIC_PRIVATE_APPROVAL_DETAIL/);
  assert.doesNotMatch(readFileSync(join(stateDir, "audit.jsonl"), "utf8"), /SYNTHETIC_PRIVATE_APPROVAL_DETAIL/);
});

test("a configured hook response missing its decision never grants execution", async () => {
  scenario({ body: { ok: true } });
  const result = await toolCall()();
  assert.equal(result?.block, true, "missing approval state cannot mean allowed");
});

test("an unsuccessful HTTP response cannot grant execution through a JSON allowed field", async () => {
  scenario({ body: { ok: true, state: "allowed" }, status: 503 });
  const result = await toolCall()();
  assert.equal(result?.block, true, "HTTP failure is not an authorization decision");
});

test("a configured disconnected hook blocks the call", async () => {
  scenario({ body: null, disconnect: true });
  assert.equal((await toolCall()())?.block, true);
});

test("malformed or extended initial responses never grant execution", async () => {
  const replies = [null, [], "allowed", true, { ok: "true", state: "allowed" },
    { ok: true, state: "unknown" }, { ok: true, state: "approved" },
    { ok: true, state: "allowed", surprise: true },
    { ok: true, state: "allowed", grantId: "" },
    { ok: true, state: "pending", approvalId: "" },
    { ok: true, state: "pending", approvalId: "approval-1", waitMs: 0 },
    { ok: true, state: "pending", approvalId: "approval-1", waitMs: 1.5 },
    { ok: true, state: "pending", approvalId: "x".repeat(257) }];
  for (const [index, body] of replies.entries()) {
    scenario({ body });
    assert.equal((await toolCall()())?.block, true, `malformed response ${index}`);
  }
});

test("a pending response without a card ID is blocked before polling", async () => {
  scenario({ body: { ok: true, state: "pending" } }, [{ body: { ok: true, state: "approved" } }]);
  assert.equal((await toolCall()())?.block, true);
  assert.deepEqual(requests, ["/hook/approval"]);
});

test("polling failures and malformed decisions cannot authorize a pending call", async () => {
  for (const body of [{ ok: false, error: "synthetic poll failure" }, { ok: true }, { ok: true, state: "allowed" }, { ok: true, state: "approved", surprise: true }]) {
    scenario(pending(), [{ body }]);
    assert.equal((await toolCall()())?.block, true);
  }
});

test("an ambiguous parked-and-approved response never runs the client call", async () => {
  scenario(pending(1), [], { body: { ok: true, state: "approved", parked: true } });
  assert.equal((await toolCall()())?.block, true);
});

test("a failed park does not turn a pending card into permission", async () => {
  scenario(pending(1), [], { body: { ok: false, error: "synthetic park failure" } });
  assert.equal((await toolCall()())?.block, true);
});

test("an oversized approval response cannot grant execution", async () => {
  const body = { ok: true, state: "allowed" };
  scenario({ body, raw: JSON.stringify(body) + " ".repeat(65536) });
  assert.equal((await toolCall()())?.block, true);
});

test("tool audit keeps identifiers and an input hash without recording private command text", async () => {
  scenario({ body: { ok: true, state: "allowed" } });
  const marker = "SYNTHETIC_PRIVATE_TOOL_COMMAND";
  assert.equal(await toolCall(extension, `printf ${marker}`)(), undefined);
  const records = readFileSync(join(stateDir, "audit.jsonl"), "utf8");
  const events = records.trim().split("\n").map((line) => JSON.parse(line));
  const call = events.filter((event) => event.kind === "tool_call").at(-1);
  assert.equal(call.tool, "bash");
  assert.equal(call.call_id, "approval-call");
  assert.match(call.input_hash, /^[a-f0-9]{16}$/);
  assert.doesNotMatch(records, /SYNTHETIC_PRIVATE_TOOL_COMMAND/);
});

test("terminal refusals cannot be reopened by a later approval response", async () => {
  const refusal = Schema.Union([
    Schema.Struct({ _tag: Schema.Literal("Denied"), approvalId: Schema.Literal("approval-1"), decision: Schema.Literals(["rejected", "cancelled", "expired"]), reason: Schema.optional(Schema.String.check(Schema.isMaxLength(64))) }),
    Schema.Struct({ _tag: Schema.Literal("Parked"), approvalId: Schema.Literal("approval-1"), reason: Schema.optional(Schema.String.check(Schema.isMaxLength(64))) }),
    Schema.Struct({ _tag: Schema.Literal("BlockedPending"), approvalId: Schema.Literal("approval-1") }),
    Schema.Struct({ _tag: Schema.Literal("Failed"), failure: Schema.Struct({ _tag: Schema.Literals(["ApprovalUnavailable", "InvalidApprovalResponse"]) }) }),
  ]);
  const attempts = Arbitrary.schema(Schema.Struct({ refusal, response: Schema.Literals(["Checked", "Polled", "ParkAnswered"]) }));
  const result = await Effect.runPromise(Arbitrary.checkEffect(attempts, ({ refusal, response }) => {
    const event: Parameters<typeof approvalTransition>[1] = response === "Checked"
      ? { _tag: "Checked", response: { ok: true, state: "allowed" }, now: 0, fallbackWaitMs: 10000 }
      : response === "Polled"
        ? { _tag: "Polled", response: { ok: true, state: "approved" } }
        : { _tag: "ParkAnswered", response: { ok: true, state: "approved", parked: false } };
    return isDeepStrictEqual(approvalTransition(refusal, event), refusal);
  }, { runs: 256, seed: 20261109 }));
  assert.equal(result._tag, "Passed", Arbitrary.formatCheckFailure(result));
  if (result._tag === "Passed") assert.equal(result.runs, 256);
});

test("approval module import or Promise failures block without exposing the error", () => {
  const fixture = mkdtempSync(join(tmpdir(), "arbos-approval-private-runtime-"));
  const moduleDir = fileURLToPath(new URL(".", import.meta.url));
  try {
    for (const name of readdirSync(moduleDir).filter((name) => name.endsWith(".ts") && !name.endsWith(".test.ts") && name !== "approval.ts")) {
      copyFileSync(join(moduleDir, name), join(fixture, name));
    }
    symlinkSync(join(moduleDir, "node_modules"), join(fixture, "node_modules"), "dir");
    writeFileSync(join(fixture, "package.json"), '{"type":"module"}');
    const script = `
      const { default: register } = await import(process.argv[1]);
      let call;
      register({ on: (name, handler) => { if (name === "tool_call") call = handler; }, registerTool() {}, appendEntry() {} });
      const result = await call({ toolName: "bash", toolCallId: "synthetic-call", input: { command: "printf approval-test" }, parentToolCallId: null });
      process.stdout.write(JSON.stringify(result));
    `;
    const run = () => JSON.parse(execFileSync(process.execPath, ["--input-type=module", "-e", script, pathToFileURL(join(fixture, "index.ts")).href], {
      encoding: "utf8", timeout: 5000,
      env: { ARBOS_HOOK_URL: `http://127.0.0.1:${port}`, ARBOS_SESSION_ROLE: "subagent", ARBOS_STATE_DIR: fixture, ARBOS_AUDIT_PATH: join(fixture, "audit.jsonl") },
    }));
    const missing = run();
    assert.equal(missing.block, true);
    assert.doesNotMatch(missing.reason, /arbos-approval-private-runtime/);
    writeFileSync(join(fixture, "approval.ts"), 'export function checkApproval() { return Promise.reject(new Error("SYNTHETIC_PRIVATE_RUNTIME_DETAIL")); }\nexport function approvalVerdict() { throw new Error("unreachable"); }\n');
    const rejected = run();
    assert.equal(rejected.block, true);
    assert.doesNotMatch(rejected.reason, /SYNTHETIC_PRIVATE_RUNTIME_DETAIL/);
    const events = readFileSync(join(fixture, "audit.jsonl"), "utf8").trim().split("\n").map((line) => JSON.parse(line));
    const errors = events.filter((event) => event.kind === "approval_error");
    assert.equal(errors.length, 2);
    assert.ok(errors.every((event) => event.error === "ApprovalUnavailable"));
    assert.doesNotMatch(JSON.stringify(errors), /SYNTHETIC_PRIVATE_RUNTIME_DETAIL|arbos-approval-private-runtime/);
  } finally {
    rmSync(fixture, { recursive: true, force: true });
  }
});

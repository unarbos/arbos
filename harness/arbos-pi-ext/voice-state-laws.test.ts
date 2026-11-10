import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, test } from "node:test";
import { fileURLToPath } from "node:url";
import { isDeepStrictEqual } from "node:util";
import * as Arbitrary from "effect/Arbitrary";
import * as Effect from "effect/Effect";
import * as Schema from "effect/Schema";

const script = fileURLToPath(new URL("../../voice/tests/state_bridge.py", import.meta.url));
const work = mkdtempSync(join(tmpdir(), "arbos-voice-laws-"));
const environment = { PATH: process.env.PATH ?? "/usr/bin:/bin" };
const args = ["-I", "-B", script];
type Machine = "vision" | "call" | "mute" | "mode";
type Event = Readonly<Record<string, string | number | boolean | readonly string[]>>;
const Reply = Schema.Struct({
  id: Schema.Int.check(Schema.isBetween({ minimum: 0, maximum: Number.MAX_SAFE_INTEGER })),
  initial: Schema.Json,
  steps: Schema.Array(Schema.Struct({ state: Schema.Json, result: Schema.Json,
    confirmed: Schema.optional(Schema.Json), requested: Schema.optional(Schema.Json) })),
});
type Reply = typeof Reply.Type;
const decode = Schema.decodeUnknownSync(Reply, { onExcessProperty: "error" });

/** One persistent pure-state child; every generated trace starts a new actual FSM. */
class StateBridge {
  private readonly child = spawn("python3", args, { cwd: work, env: environment, stdio: "pipe" });
  private readonly exited: Promise<void>;
  private tail: Promise<unknown> = Promise.resolve();
  private nextId = 0;
  private failed: Error | undefined;
  private buffer = "";
  private closing = false;
  private pending: { id: number; count: number; resolve: (reply: Reply) => void; reject: (error: Error) => void; timer: NodeJS.Timeout } | undefined;

  constructor() {
    this.exited = new Promise((resolve) => {
      this.child.once("close", () => { if (!this.closing) this.fail(); resolve(); });
      this.child.once("error", () => { this.fail(); resolve(); });
    });
    this.child.stdin.on("error", () => this.fail());
    this.child.stderr.on("data", () => {});
    this.child.stdout.setEncoding("utf8");
    this.child.stdout.on("data", (chunk: string) => {
      if (this.failed || this.closing) return;
      this.buffer += chunk;
      if (Buffer.byteLength(this.buffer) > 262144) { this.fail(); return; }
      for (;;) {
        const end = this.buffer.indexOf("\n");
        if (end < 0) return;
        const line = this.buffer.slice(0, end);
        this.buffer = this.buffer.slice(end + 1);
        try {
          const pending = this.pending;
          const reply = decode(JSON.parse(line));
          if (!pending || reply.id !== pending.id || reply.steps.length !== pending.count) throw new Error("Invalid response shape");
          clearTimeout(pending.timer);
          this.pending = undefined;
          pending.resolve(reply);
        } catch { this.fail(); return; }
      }
    });
  }

  request(machine: Machine, events: readonly Event[]): Promise<Reply> {
    const next = this.tail.then(() => {
      if (this.failed || this.closing) throw new Error("Pure voice oracle unavailable");
      const id = this.nextId++;
      const line = JSON.stringify({ id, machine, events });
      if (events.length > 64 || Buffer.byteLength(line) > 65536) throw new Error("Trace exceeds protocol bound");
      return new Promise<Reply>((resolve, reject) => {
        const timer = setTimeout(() => this.fail(), 5000);
        this.pending = { id, count: events.length, resolve, reject, timer };
        this.child.stdin.write(line + "\n");
      });
    });
    this.tail = next.catch(() => undefined);
    return next;
  }

  private fail(): void {
    this.failed ??= new Error("Pure voice oracle failed");
    if (this.pending) {
      clearTimeout(this.pending.timer);
      this.pending.reject(this.failed);
      this.pending = undefined;
    }
    this.child.kill();
  }

  async close(): Promise<void> {
    this.closing = true;
    this.child.stdin.end();
    const timer = setTimeout(() => this.child.kill("SIGKILL"), 1000);
    try { await this.exited; } finally { clearTimeout(timer); }
  }
}

const bridge = new StateBridge();
after(async () => { await bridge.close(); rmSync(work, { recursive: true, force: true }); });
const reference = Schema.Int.check(Schema.isBetween({ minimum: 0, maximum: 7 }));
const reason = Schema.Literals(["asked", "shutdown", "room_closed"]);
const visionEvent = Schema.Union([
  Schema.Struct({ kind: Schema.Literals(["enable", "disable", "close", "capture"]) }),
  Schema.Struct({ kind: Schema.Literal("frame"), token: reference }),
]);
const callEvent = Schema.Union([
  Schema.Struct({ kind: Schema.Literal("progress"), stage: Schema.Literals(["waiting", "starting", "running"]) }),
  Schema.Struct({ kind: Schema.Literals(["stop", "prestop"]), reason }),
  Schema.Struct({ kind: Schema.Literal("finalize"), token: reference }),
]);
const muteEvent = Schema.Union([
  Schema.Struct({ kind: Schema.Literal("request"), value: Schema.Boolean }),
  Schema.Struct({ kind: Schema.Literals(["success", "error"]), token: reference }),
]);
const modeEvent = Schema.Union([
  Schema.Struct({ kind: Schema.Literal("request"), value: Schema.Literals(["group", "alice", "bob"]) }),
  Schema.Struct({ kind: Schema.Literals(["success", "error"]), token: reference }),
]);
const input = Schema.Struct({
  vision: Schema.Array(visionEvent).check(Schema.isMaxLength(24)),
  call: Schema.Array(callEvent).check(Schema.isMaxLength(24)),
  mute: Schema.Array(muteEvent).check(Schema.isMaxLength(24)),
  mode: Schema.Array(modeEvent).check(Schema.isMaxLength(24)),
  firstReason: reason, lastReason: reason, initialMute: Schema.Boolean,
  failedStep: Schema.Literals(["listeners", "vision", "updates", "session", "tasks"]),
});
type Input = typeof input.Type;
const canonical: Input = { vision: [], call: [], mute: [], mode: [], firstReason: "asked", lastReason: "shutdown", initialMute: false, failedStep: "vision" };
const arbitrary = Arbitrary.schema(input, { shrink: (value) => isDeepStrictEqual(value, canonical) ? [] : [canonical] });
const equal = isDeepStrictEqual;
function snapshot(value: Schema.Json): Record<string, Schema.Json> {
  assert.ok(value !== null && typeof value === "object" && !Array.isArray(value));
  const object = value as Record<string, Schema.Json>;
  assert.equal(typeof object._tag, "string");
  return object;
}
function monotonic(reply: Reply, field: "generation" | "attempt"): boolean {
  const values = [reply.initial, ...reply.steps.map((step) => step.state)].map((state) => snapshot(state)[field] ?? 0);
  return values.every((value, i) => typeof value === "number" && Number.isSafeInteger(value) && value >= 0
    && (i === 0 || value >= (values[i - 1] as number)));
}
function law(name: string, seed: number, predicate: (value: Input) => Promise<boolean>) {
  test(`${name}: 256 seeded traces`, async () => {
    const result = await Effect.runPromise(Arbitrary.checkEffect(arbitrary, (value) => Effect.tryPromise({
      try: () => predicate(value), catch: () => new Error("Voice state observation failed"),
    }), { runs: 256, seed, size: 64, maxShrinks: 256, concurrency: 1 }));
    assert.equal(result._tag, "Passed", Arbitrary.formatCheckFailure(result));
    if (result._tag === "Passed") assert.equal(result.runs, 256);
  });
}

law("closed call and vision lifetimes cannot restart", 84001, async (value) => {
  const call = await bridge.request("call", [{ kind: "stop", reason: value.firstReason }, { kind: "finalize", token: 0 }, { kind: "progress", stage: "running" }, ...value.call]);
  const vision = await bridge.request("vision", [{ kind: "close" }, { kind: "enable" }, ...value.vision]);
  const terminal = call.steps[1].state;
  return snapshot(terminal)._tag === "Closed" && call.steps.slice(1).every((step) => equal(step.state, terminal))
    && snapshot(vision.steps[0].state)._tag === "Closed" && vision.steps.every((step) => equal(step.state, vision.steps[0].state));
});

law("revoked frame leases stay invalid after reenable", 84002, async (value) => {
  const reply = await bridge.request("vision", [
    { kind: "capture" }, { kind: "disable" }, { kind: "enable" }, { kind: "frame", token: 0 },
    { kind: "capture" }, { kind: "frame", token: 1 }, ...value.vision,
  ]);
  return reply.steps[0].result !== null && reply.steps[3].result === false && reply.steps[5].result === true
    && reply.steps[0].result !== reply.steps[4].result && monotonic(reply, "generation");
});

law("failed cleanup retains ownership and the original stop reason", 84003, async (value) => {
  const reply = await bridge.request("call", [
    { kind: "stop", reason: value.firstReason }, { kind: "finalize", token: 0, failed: [value.failedStep] },
    { kind: "stop", reason: value.lastReason }, { kind: "finalize", token: 0 }, { kind: "finalize", token: 1 }, ...value.call,
  ]);
  const failed = snapshot(reply.steps[1].state), retry = snapshot(reply.steps[2].state), closed = snapshot(reply.steps[4].state);
  return failed._tag === "CleanupFailed" && equal(failed.pending_steps, [value.failedStep])
    && retry._tag === "Finishing" && equal(retry.pending_steps, [value.failedStep])
    && equal(reply.steps[2].state, reply.steps[3].state) && closed._tag === "Closed"
    && [failed, retry, closed].every((state) => state.reason === value.firstReason) && monotonic(reply, "attempt");
});

law("failed mute is not committed and can retry the same target", 84004, async (value) => {
  const reply = await bridge.request("mute", [
    { kind: "request", value: value.initialMute }, { kind: "success", token: 0 },
    { kind: "request", value: !value.initialMute }, { kind: "error", token: 1 },
    { kind: "request", value: !value.initialMute }, { kind: "success", token: 2 }, ...value.mute,
  ]);
  return snapshot(reply.steps[3].state)._tag === "ApplyFailed" && reply.steps[3].confirmed === value.initialMute
    && reply.steps[3].requested === !value.initialMute && snapshot(reply.steps[4].state)._tag === "Applying"
    && (snapshot(reply.steps[4].state).attempt as number) > (snapshot(reply.steps[3].state).attempt as number)
    && reply.steps[5].confirmed === !value.initialMute && monotonic(reply, "attempt");
});

for (const [offset, machine, a, b] of [[0, "mute", false, true], [1, "mode", "group", "alice"]] as const) {
  law(`old ${machine} completions cannot commit over a newer request`, 84005 + offset, async (value) => {
    const reply = await bridge.request(machine, [
      { kind: "request", value: a }, { kind: "request", value: b },
      { kind: "success", token: 0 }, { kind: "error", token: 0 }, { kind: "success", token: 1 }, ...value[machine],
    ]);
    const expected = machine === "mode" ? { _tag: "Duplex", identity: "u_alice" } : true;
    return equal(reply.steps[1].state, reply.steps[2].state) && equal(reply.steps[2].state, reply.steps[3].state)
      && snapshot(reply.steps[4].state)._tag === "Applied" && equal(reply.steps[4].requested, expected) && equal(reply.steps[4].confirmed, expected)
      && monotonic(reply, "attempt");
  });
}

test("pure trace protocol rejects malformed, oversized and wrong-type inputs", () => {
  const bad = ["{", JSON.stringify({ id: 0, machine: "unknown", events: [] }),
    JSON.stringify({ id: 1, machine: "mute", events: [{ kind: "request", value: "false" }] }),
    JSON.stringify({ id: 2, machine: "vision", events: [{ kind: "frame", token: -1 }] }),
    JSON.stringify({ id: 3, machine: "vision", events: Array(65).fill({ kind: "close" }) }),
    JSON.stringify({ id: 4, machine: "vision", events: [], extra: true }), "x".repeat(65537)];
  const child = spawnSync("python3", args, { cwd: work, env: environment, input: bad.join("\n") + "\n", encoding: "utf8", timeout: 5000 });
  assert.equal(child.status, 0);
  assert.equal(child.stderr, "");
  const replies = child.stdout.trim().split("\n").map((line) => JSON.parse(line));
  assert.equal(replies.length, bad.length);
  assert.ok(replies.every((reply) => reply.error === "Invalid trace" && Object.keys(reply).length === 2));
});

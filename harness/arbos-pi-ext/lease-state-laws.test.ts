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

const script = fileURLToPath(new URL("../../scripts/runner/tests/lease_state_bridge.py", import.meta.url));
const directory = mkdtempSync(join(tmpdir(), "arbos-lease-laws-"));
const environment = { PATH: process.env.PATH ?? "/usr/bin:/bin" };
const python = ["-I", "-B", script];
type Machine = "heavy" | "lease" | "gui" | "boot";
type Event = { event: string; args: readonly string[] };
const Measurement = Schema.Struct({ status: Schema.Int, stdout: Schema.String });
const Observation = Schema.Struct({ state: Schema.String, status: Schema.Int,
  projection: Schema.optional(Measurement), constructed: Schema.optional(Measurement) });
const Reply = Schema.Struct({ id: Schema.Int, initial: Observation, steps: Schema.Array(Observation) });
type Reply = typeof Reply.Type;
const decode = Schema.decodeUnknownSync(Reply, { onExcessProperty: "error" });

class Bridge {
  private readonly child = spawn("python3", python, { cwd: directory, env: environment, stdio: "pipe" });
  private readonly exited: Promise<void>;
  private sequence = 0;
  private buffer = "";
  private closing = false;
  private failed = false;
  private tail: Promise<unknown> = Promise.resolve();
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
      this.buffer += chunk;
      if (Buffer.byteLength(this.buffer) > 262144) { this.fail(); return; }
      for (;;) {
        const at = this.buffer.indexOf("\n");
        if (at < 0) return;
        const line = this.buffer.slice(0, at); this.buffer = this.buffer.slice(at + 1);
        try {
          const reply = decode(JSON.parse(line));
          const pending = this.pending;
          if (!pending || reply.id !== pending.id || reply.steps.length !== pending.count) throw new Error("Invalid reply");
          clearTimeout(pending.timer); this.pending = undefined; pending.resolve(reply);
        } catch { this.fail(); return; }
      }
    });
  }
  request(machine: Machine, initial: string, events: readonly Event[]): Promise<Reply> {
    const next = this.tail.then(() => {
      if (this.failed || this.closing) throw new Error("Pure lease oracle unavailable");
      const id = this.sequence++;
      const line = JSON.stringify({ id, machine, initial, events });
      if (events.length > 64 || Buffer.byteLength(line) > 65536) throw new Error("Trace exceeds bound");
      return new Promise<Reply>((resolve, reject) => {
        this.pending = { id, count: events.length, resolve, reject, timer: setTimeout(() => this.fail(), 5000) };
        this.child.stdin.write(line + "\n");
      });
    });
    this.tail = next.catch(() => undefined);
    return next;
  }
  private fail(): void {
    this.failed = true;
    if (this.pending) {
      clearTimeout(this.pending.timer); this.pending.reject(new Error("Pure lease observation failed")); this.pending = undefined;
    }
    this.child.kill();
  }
  async close(): Promise<void> {
    this.closing = true; this.child.stdin.end();
    const timer = setTimeout(() => this.child.kill("SIGKILL"), 1000);
    try { await this.exited; } finally { clearTimeout(timer); }
  }
}

const bridge = new Bridge();
after(async () => { await bridge.close(); rmSync(directory, { recursive: true, force: true }); });
const Input = Schema.Struct({
  ask: Schema.Literals(["Ask", "Queue"]),
  phase: Schema.Literals(["Claiming", "Prepared", "Verified", "Published"]),
  abort: Schema.Literals(["Cancelled", "OwnerGone", "Unavailable"]),
  badPid: Schema.Literals(["0", "1", "invalid", "9999999999999999999"]),
  badSince: Schema.Literals(["-1", "invalid", "9999999999999999999"]),
  snapshot: Schema.Literals(["Absent", "Unpublished", "Dead", "Retired"]),
  scope: Schema.Literals(["Ancestor", "Explicit"]),
  tail: Schema.Array(Schema.Literals(["Prepared", "OwnerStatus", "Published", "Stored", "Removed", "Count", "OwnedLease", "Workflow", "Cancelled"])).check(Schema.isMaxLength(6)),
});
type Input = typeof Input.Type;
const canonical: Input = { ask: "Ask", phase: "Claiming", abort: "Cancelled", badPid: "1", badSince: "invalid", snapshot: "Retired", scope: "Explicit", tail: [] };
const arbitrary = Arbitrary.schema(Input, { shrink: (value) => isDeepStrictEqual(value, canonical) ? [] : [canonical] });
const event = (name: string, ...args: string[]): Event => ({ event: name, args });
const path = "/fixture/lease.slot";
const forward = [event("Prepared", "100", "rig", "1700000000"), event("OwnerStatus", "alive"), event("Published"), event("OwnerStatus", "alive")];
const traces = (value: Input) => value.tail.map((name) => event(name, "alive", "rig", "1700000000"));
const successful = (reply: Reply) => reply.steps.every((step) => step.status === 0);
const last = (reply: Reply) => reply.steps.at(-1)!;

function law(name: string, seed: number, predicate: (value: Input) => Promise<boolean>) {
  if (process.env.ARBOS_LEASE_LAW && process.env.ARBOS_LEASE_LAW !== String(seed)) return;
  test(`${name}: 256 seeded cases`, async () => {
    if (process.env.ARBOS_LEASE_REPLAY) {
      const value = Schema.decodeUnknownSync(Input, { onExcessProperty: "error" })(JSON.parse(process.env.ARBOS_LEASE_REPLAY));
      assert.equal(await predicate(value), true, `seed=${seed} canonical replay=${JSON.stringify(value)}`);
      return;
    }
    const result = await Effect.runPromise(Arbitrary.checkEffect(arbitrary,
      (value) => Effect.tryPromise({ try: () => predicate(value), catch: () => new Error("Pure lease observation failed") }),
      { runs: 256, seed, size: 64, maxShrinks: 256, concurrency: 1 }));
    assert.equal(result._tag, "Passed", Arbitrary.formatCheckFailure(result));
    if (result._tag === "Passed") assert.equal(result.runs, 256);
  });
}

law("metadata and both owner observations precede a heavy grant", 89001, async (value) => {
  const prefix = [event(value.ask), event("Claim", path)];
  for (const args of [[value.badPid, "rig", "1700000000"], ["100", "", "1700000000"], ["100", "rig", value.badSince]]) {
    const reply = await bridge.request("heavy", "New", [...prefix, event("Prepared", ...args), ...forward.slice(1), ...traces(value)]);
    if (!successful(reply) || reply.steps.slice(2).some((step) => step.state !== "Unavailable")) return false;
  }
  const valid = await bridge.request("heavy", "New", [...prefix, ...forward]);
  for (const before of [1, 3]) {
    const dead = await bridge.request("heavy", "New", [...prefix, ...forward.slice(0, before), event("OwnerStatus", "dead"), ...forward, ...traces(value)]);
    if (!successful(dead) || dead.steps.slice(prefix.length + before).some((step) => step.state !== "Abandoned")) return false;
  }
  return successful(valid) && last(valid).state === `Granted:${path}`
    && valid.steps.slice(0, -1).every((step) => !step.state.startsWith("Granted:"));
});

law("abandoned heavy and GUI attempts reject late completion", 89002, async (value) => {
  const phases = ["Claiming", "Prepared", "Verified", "Published"] as const;
  const at = phases.indexOf(value.phase);
  const prefix = [event(value.ask), event("Claim", path), ...forward.slice(0, at)];
  const request = await bridge.request("heavy", "New", [...prefix, event(value.abort), ...forward, ...traces(value)]);
  const terminal = request.steps[prefix.length]?.state;
  if (!successful(request) || !["Cancelled", "Abandoned", "Unavailable"].includes(terminal ?? "")
      || request.steps.slice(prefix.length).some((step) => step.state !== terminal)) return false;
  const gui = await bridge.request("gui", "Free", [event("Acquire"), event("Cancelled"), event("Stored"), ...traces(value)]);
  const failedGui = await bridge.request("gui", "Free", [event("Acquire"), event("OwnerGone"), event("Stored"), ...traces(value)]);
  const validGui = await bridge.request("gui", "Free", [event("Acquire"), event("Stored")]);
  return successful(gui) && gui.steps.slice(1).every((step) => step.state === "Cancelled")
    && successful(failedGui) && failedGui.steps.slice(1).every((step) => step.state === "Failed")
    && successful(validGui) && last(validGui).state === "Granted";
});

law("invalid or failed-retirement leases keep capacity reserved", 89003, async (value) => {
  const invalid = await bridge.request("lease", "Observed", [event("Invalid"), ...traces(value)]);
  if (!successful(invalid) || invalid.steps.some((step) => step.state !== "InvalidHeld"
      || step.projection?.status !== 0 || step.projection.stdout !== "1")) return false;
  const failed = await bridge.request("lease", "ExpiredRig", [event("MarkFailed"), event("RemoveFailed"), ...traces(value)]);
  const retired = await bridge.request("lease", "ExpiredRig", [event("Marked"), event("RemoveFailed")]);
  return successful(failed) && last(failed).state === "HeldCleanupFailed"
    && last(failed).projection?.status === 0 && last(failed).projection?.stdout === "1"
    && successful(retired) && last(retired).state === "RetiredCleanupFailed"
    && last(retired).projection?.status === 0 && last(retired).projection?.stdout === "0";
});

law("invalid retired or rejected boot observations cannot authorize", 89004, async (value) => {
  const denied = await bridge.request("boot", "Checking", [event("Observe", value.snapshot, value.scope, path), event("NoLease"), event("OwnedLease", path), ...traces(value)]);
  const valid = await bridge.request("boot", "Checking", [event("Observe", "Owned", "Ancestor", path)]);
  const explicit = await bridge.request("boot", "Checking", [event("Observe", "External", "Explicit", path)]);
  return successful(denied) && denied.steps[0]?.constructed?.stdout === "RejectedLease"
    && denied.steps.slice(1).every((step) => step.state === "Denied")
    && successful(valid) && last(valid).state === `Authorized:${path}`
    && successful(explicit) && last(explicit).state === `Authorized:${path}`;
});

test("pure lease protocol rejects bad frames and recovers", () => {
  const bad = ["{", JSON.stringify({ id: 0, machine: "unknown", initial: "New", events: [] }),
    JSON.stringify({ id: 1, machine: "heavy", initial: "New", events: [{ event: "Ask", args: [1] }] }),
    JSON.stringify({ id: 2, machine: "heavy", initial: "New", events: Array(65).fill(event("Ask")) }),
    JSON.stringify({ id: 3, machine: "gui", initial: "Free", events: [], extra: true }), "x".repeat(65537)];
  const good = JSON.stringify({ id: 4, machine: "gui", initial: "Free", events: [event("Acquire"), event("Stored")] });
  const child = spawnSync("python3", python, { cwd: directory, env: environment, input: [...bad, good].join("\n") + "\n", encoding: "utf8", timeout: 5000 });
  assert.equal(child.status, 0); assert.equal(child.stderr, "");
  const replies = child.stdout.trim().split("\n").map((line) => JSON.parse(line));
  assert.equal(replies.length, bad.length + 1);
  assert.ok(replies.slice(0, -1).every((reply) => reply.error === "Invalid trace" && Object.keys(reply).length === 2));
  assert.equal(decode(replies.at(-1)).steps.at(-1)?.state, "Granted");
});

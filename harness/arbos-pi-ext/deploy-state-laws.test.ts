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

const script = fileURLToPath(new URL("../../scripts/deploy/tests/deploy_state_bridge.py", import.meta.url));
const directory = mkdtempSync(join(tmpdir(), "arbos-deploy-laws-"));
const environment = { PATH: process.env.PATH ?? "/usr/bin:/bin" };
const python = ["-I", "-B", script];
type Machine = "request" | "lane" | "publication";
type Event = { event: string; args?: readonly string[] };
const Observation = Schema.Struct({ state: Schema.String, status: Schema.Int });
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
      if (this.failed || this.closing) throw new Error("Pure deployment oracle unavailable");
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
      clearTimeout(this.pending.timer); this.pending.reject(new Error("Pure deployment observation failed")); this.pending = undefined;
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
const Events = Schema.Literals(["Unchanged", "Changed", "Locked", "Busy", "Clear", "Blocked", "Finished", "Unknown"]);
const Input = Schema.Struct({
  outcome: Schema.Literals(["Unchanged", "Changed"]),
  terminal: Schema.Literals(["Deferred", "Released"]),
  tail: Schema.Array(Events).check(Schema.isMaxLength(16)),
});
type Input = typeof Input.Type;
const canonical: Input = { outcome: "Unchanged", terminal: "Released", tail: [] };
const arbitrary = Arbitrary.schema(Input, { shrink: (value) => isDeepStrictEqual(value, canonical) ? [] : [canonical] });
const events = (...names: readonly string[]): Event[] => names.map((event) => ({ event }));

function law(name: string, seed: number, predicate: (value: Input) => Promise<boolean>) {
  if (process.env.ARBOS_DEPLOY_LAW && process.env.ARBOS_DEPLOY_LAW !== String(seed)) return;
  test(`${name}: 256 seeded cases`, async () => {
    if (process.env.ARBOS_DEPLOY_REPLAY) {
      const value = Schema.decodeUnknownSync(Input, { onExcessProperty: "error" })(JSON.parse(process.env.ARBOS_DEPLOY_REPLAY));
      assert.equal(await predicate(value), true, `seed=${seed} canonical replay=${JSON.stringify(value)}`);
      return;
    }
    const result = await Effect.runPromise(Arbitrary.checkEffect(arbitrary,
      (value) => Effect.tryPromise({ try: () => predicate(value), catch: () => new Error("Pure deployment observation failed") }),
      { runs: 256, seed, size: 64, maxShrinks: 256, concurrency: 1 }));
    assert.equal(result._tag, "Passed", Arbitrary.formatCheckFailure(result));
    if (result._tag === "Passed") assert.equal(result.runs, 256);
  });
}

law("a replaced request can never be acknowledged by an old receipt", 89701, async (value) => {
  const replaced = await bridge.request("request", "Observed", events("Changed", value.outcome, ...value.tail));
  const current = await bridge.request("request", "Observed", events("Unchanged", ...value.tail));
  return replaced.steps.every((step) => step.status === 0 && step.state === "Superseded")
    && current.steps.every((step) => step.status === 0 && step.state === "Acknowledged");
});

law("grant requires the lock followed by a clear blocker observation", 89702, async (value) => {
  const traces = [value.tail, ["Busy", ...value.tail], ["Locked", "Blocked", ...value.tail], ["Clear", ...value.tail]];
  for (const trace of traces) {
    const reply = await bridge.request("lane", "Checking", events(...trace));
    for (let index = 0; index < reply.steps.length; index++) {
      const step = reply.steps[index]!;
      if (step.state === "Granted") {
        if (trace[index] !== "Clear" || index === 0 || reply.steps[index - 1]!.state !== "Inspecting"
            || trace[index - 1] !== "Locked" || reply.steps[index - 1]!.status !== 0) return false;
      }
    }
  }
  const admitted = await bridge.request("lane", "Checking", events("Locked", "Clear", "Finished"));
  return admitted.steps.map((step) => step.state).join(",") === "Inspecting,Granted,Released"
    && admitted.steps.every((step) => step.status === 0);
});

law("deferred and released lane owners cannot authorize late work", 89703, async (value) => {
  for (const terminal of [value.terminal, "Deferred", "Released"]) {
    const reply = await bridge.request("lane", terminal, events("Locked", "Clear", ...value.tail));
    if (reply.steps.some((step) => step.status !== 0 || step.state !== terminal)) return false;
  }
  return true;
});

const PublicationInput = Schema.Struct({
  kind: Schema.Literals(["relay", "voice", "agentd"]),
  revision: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0), Schema.isLessThanOrEqualTo(1000000)),
  queued: Schema.Literals(["-", "aaaaaaa", "bbbbbbb"]),
  installed: Schema.Literals(["-", "aaaaaaa", "bbbbbbb"]),
  currentDefault: Schema.Literals(["-", "aaaaaaa", "bbbbbbb"]),
});
type PublicationInput = typeof PublicationInput.Type;
const publicationCanonical: PublicationInput = { kind: "relay", revision: 0, queued: "-", installed: "-", currentDefault: "-" };
const publicationArbitrary = Arbitrary.schema(PublicationInput, { shrink: (value) => isDeepStrictEqual(value, publicationCanonical) ? [] : [publicationCanonical] });
async function publicationLaw(value: PublicationInput): Promise<boolean> {
  const fields = [value.kind, String(value.revision), value.queued, value.installed, value.currentDefault];
  const snapshot = fields.join(":");
  const observe = async (expected: string, actual: string) =>
    (await bridge.request("publication", "Checking", [{ event: expected, args: [actual] }])).steps[0]!;
  const current = await observe(snapshot, snapshot);
  if (current.status !== 0 || current.state !== "Publish") return false;
  for (let index = 0; index < fields.length; index++) {
    const changed = [...fields];
    changed[index] = index === 0 ? (value.kind === "relay" ? "voice" : "relay")
      : index === 1 ? String(value.revision + 2) : fields[index] === "aaaaaaa" ? "bbbbbbb" : "aaaaaaa";
    const stale = await observe(snapshot, changed.join(":"));
    if (stale.status !== 0 || stale.state !== "Superseded") return false;
  }
  const malformed = await observe(snapshot + ":extra", snapshot);
  return malformed.status !== 0 && malformed.state === "Invalid";
}

if (!process.env.ARBOS_DEPLOY_LAW || process.env.ARBOS_DEPLOY_LAW === "89704") {
  test("every observed publication field binds admission, including ABA revision: 256 seeded cases", async () => {
    if (process.env.ARBOS_DEPLOY_REPLAY) {
      const value = Schema.decodeUnknownSync(PublicationInput, { onExcessProperty: "error" })(JSON.parse(process.env.ARBOS_DEPLOY_REPLAY));
      assert.equal(await publicationLaw(value), true, `seed=89704 canonical replay=${JSON.stringify(value)}`);
      return;
    }
    const result = await Effect.runPromise(Arbitrary.checkEffect(publicationArbitrary,
      (value) => Effect.tryPromise({ try: () => publicationLaw(value), catch: () => new Error("Pure publication observation failed") }),
      { runs: 256, seed: 89704, size: 64, maxShrinks: 256, concurrency: 1 }));
    assert.equal(result._tag, "Passed", Arbitrary.formatCheckFailure(result));
    if (result._tag === "Passed") assert.equal(result.runs, 256);
  });
}

test("pure deployment protocol rejects invalid input and recovers", () => {
  const bad = ["{", JSON.stringify({ id: 0, machine: "unknown", initial: "Observed", events: [] }),
    JSON.stringify({ id: 1, machine: "request", initial: "Observed", events: [{ event: "Changed", extra: true }] })];
  const good = { id: 7, machine: "lane", initial: "Checking", events: [{ event: "Locked" }, { event: "Clear" }] };
  const result = spawnSync("python3", python, { cwd: directory, env: environment, input: [...bad, JSON.stringify(good)].join("\n") + "\n", encoding: "utf8", timeout: 5000 });
  assert.equal(result.status, 0, result.stderr);
  const rows = result.stdout.trim().split("\n").map((line) => JSON.parse(line) as Record<string, unknown>);
  assert.equal(rows.length, 4);
  assert.ok(rows.slice(0, -1).every((row) => row.error === "Invalid trace"));
  const reply = decode(rows.at(-1));
  assert.equal(reply.id, 7);
  assert.equal(reply.steps.at(-1)!.state, "Granted");
});

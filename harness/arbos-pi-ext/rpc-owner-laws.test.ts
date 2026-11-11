import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, test } from "node:test";
import { fileURLToPath } from "node:url";
import { isDeepStrictEqual } from "node:util";
import * as Arbitrary from "effect/Arbitrary";
import * as Effect from "effect/Effect";
import * as Schema from "effect/Schema";

const source = fileURLToPath(new URL("../../.improve/state-machine-checks/pi_slot_bridge.rs", import.meta.url));
const work = mkdtempSync(join(tmpdir(), "arbos-pi-slot-laws-"));
const environment = { PATH: process.env.PATH ?? "/usr/bin:/bin" };
const binary = join(work, process.platform === "win32" ? "pi-slot-bridge.exe" : "pi-slot-bridge");
const product = fileURLToPath(new URL("../../crates/arbos-agentd/src/pi_slot.rs", import.meta.url));
const hash = (file: string) => createHash("sha256").update(readFileSync(file)).digest("hex");
const sources = { product: hash(product), bridge: hash(source) };
const build = spawnSync("rustc", ["--edition=2021", "--crate-name", "pi_slot_bridge", "-O", source, "-o", binary],
  { cwd: work, env: environment, encoding: "utf8", timeout: 30000 });
assert.equal(build.status, 0, "Unable to compile the actual pure main-Pi slot bridge");
const snapshot = Schema.Struct({ tag: Schema.Literals(["Vacant", "Starting", "Checking", "Ready", "Exited", "Parked", "Stopped"]),
  child: Schema.NullOr(Schema.Int), owner: Schema.NullOr(Schema.Int),
  action: Schema.Literals(["Started", "Accepted", "Retired", "Rejected"]), released: Schema.NullOr(Schema.Int) });
const Reply = Schema.Struct({ id: Schema.Int.check(Schema.isBetween({ minimum: 0, maximum: 4294967295 })),
  steps: Schema.Array(snapshot) });
type Reply = typeof Reply.Type;
const decode = Schema.decodeUnknownSync(Reply, { onExcessProperty: "error" });

/** Every request starts with a fresh actual resource slot; only local integer handles are used. */
class PiSlotBridge {
  private readonly child = spawn(binary, [], { cwd: work, env: environment, stdio: "pipe" });
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

  request(events: readonly string[]): Promise<Reply> {
    const next = this.tail.then(() => {
      if (this.failed || this.closing) throw new Error("PiSlot observation bridge unavailable");
      const id = this.nextId++;
      const line = `${id}|${events.join(",")}`;
      if (events.length > 64 || Buffer.byteLength(line) > 8192) throw new Error("Trace exceeds protocol bound");
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
    this.failed ??= new Error("PiSlot observation bridge failed");
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

const bridge = new PiSlotBridge();
after(async () => {
  try { await bridge.close(); assert.deepEqual({ product: hash(product), bridge: hash(source) }, sources); }
  finally { rmSync(work, { recursive: true, force: true }); }
});
const input = Schema.Struct({ tail: Schema.Array(Schema.Int.check(Schema.isBetween({ minimum: 0, maximum: 8 }))).check(Schema.isMaxLength(24)) });
type Input = typeof input.Type;
const canonical: Input = { tail: [] };
const arbitrary = Arbitrary.schema(input, { shrink: (value) => isDeepStrictEqual(value, canonical) ? [] : [canonical] });
function law(name: string, seed: number, predicate: (value: Input) => Promise<boolean>) {
  if (process.env.ARBOS_RPC_LAW && process.env.ARBOS_RPC_LAW !== String(seed)) return;
  test(`${name}: 256 seeded traces`, async () => {
    const property = (value: Input) => Effect.tryPromise({ try: () => predicate(value), catch: () => new Error("PiSlot observation failed") });
    if (process.env.ARBOS_RPC_REPLAY) {
      const value = Schema.decodeUnknownSync(input, { onExcessProperty: "error" })(JSON.parse(process.env.ARBOS_RPC_REPLAY));
      assert.equal(await predicate(value), true, "Exact counterexample replay"); return;
    }
    const result = await Effect.runPromise(Arbitrary.checkEffect(arbitrary, property,
      { runs: 256, seed, size: 64, maxShrinks: 256, concurrency: 1 }));
    if (result._tag === "Falsified") {
      const replay = await Effect.runPromise(Arbitrary.checkEffect(arbitrary, property, { replay: result.replay }));
      assert.equal(replay._tag, "Falsified");
      if (replay._tag === "Falsified") assert.deepEqual(replay.shrunkInput, result.shrunkInput);
      console.log("pi-slot-counterexample:" + Buffer.from(JSON.stringify({ law: name, seed, result, replay })).toString("base64"));
    }
    assert.equal(result._tag, "Passed", Arbitrary.formatCheckFailure(result));
    if (result._tag === "Passed") assert.equal(result.runs, 256);
  });
}

law("stale boot callbacks cannot retire or publish a replacement", 91001, async (value) => {
  const old = ["r:1", "x:1", "p:1", "t:1", "e:1"];
  const prefix = ["b", "a:1:11", "r:1", "b", "a:2:22"];
  const stale = ["x:1", "r:1", ...value.tail.map((n) => old[n % old.length])];
  const reply = await bridge.request([...prefix, ...stale, "r:2", "e:2", "p:2"]);
  return reply.steps[2].tag === "Ready" && reply.steps[2].child === 11
    && reply.steps[3].released === 11
    && reply.steps.slice(5, 5 + stale.length).every((s) => s.tag === "Checking" && s.child === 22 && s.owner === 2 && s.action === "Rejected" && s.released === null)
    && reply.steps.at(-3)?.tag === "Ready" && reply.steps.at(-3)?.child === 22
    && reply.steps.at(-2)?.tag === "Exited" && reply.steps.at(-2)?.child === 22
    && reply.steps.at(-1)?.tag === "Parked" && reply.steps.at(-1)?.released === 22;
});

law("terminal shutdown rejects every late admission and completion", 91002, async (value) => {
  const events = ["b", "a:1:11", "r:1", "s", ...value.tail.map((n, i) => ["b", "r:1", "x:1", `a:1:${100+i}`, "e:1", "t:1", "p:1", "s", "b"][n]), "a:1:99", "b"];
  const reply = await bridge.request(events);
  return reply.steps[2].tag === "Ready" && reply.steps[3].released === 11
    && reply.steps.slice(3).every((s) => s.tag === "Stopped" && s.child === null && s.owner === null && s.action !== "Started")
    && reply.steps.at(-2)?.action === "Rejected" && reply.steps.at(-2)?.released === 99;
});

law("every attached resource is returned exactly once by retirement or rejection", 91003, async (value) => {
  const commands = value.tail.map((n, i) => ["b", `a:0:${i+1}`, "r:0", "e:0", "t:0", "p:0", "x:0", "s", `a:1:${i+1}`][n]);
  const events = ["b", "a:1:99", "r:1", ...commands, "s"];
  const attached = events.filter((e) => e.startsWith("a:")).map((e) => Number(e.split(":")[2])).sort((a,b) => a-b);
  const reply = await bridge.request(events);
  const returned = reply.steps.flatMap((s) => s.released === null ? [] : [s.released]).sort((a,b) => a-b);
  return reply.steps[1].tag === "Checking" && reply.steps[1].child === 99
    && reply.steps[2].tag === "Ready" && reply.steps[2].child === 99
    && isDeepStrictEqual(returned, attached) && reply.steps.at(-1)?.tag === "Stopped" && reply.steps.at(-1)?.child === null;
});

test("bounded native slot protocol rejects malformed traces and recovers", () => {
  const bad = ["broken", "1|unknown", "1|a:1:bad", "1|r:1", "1|" + Array(65).fill("b").join(","), "x".repeat(8193)];
  const child = spawnSync(binary, [], { cwd: work, env: environment,
    input: [...bad, "9|b,a:1:11,r:1,s"].join("\n") + "\n", encoding: "utf8", timeout: 5000 });
  assert.equal(child.status, 0); assert.equal(child.stderr, "");
  const replies = child.stdout.trim().split("\n").map((line) => JSON.parse(line));
  assert.equal(replies.length, bad.length + 1);
  assert.ok(replies.slice(0, -1).every((r) => r.id === null && r.error === "Invalid trace" && Object.keys(r).length === 2));
  const reply = decode(replies.at(-1)); assert.equal(reply.id, 9); assert.equal(reply.steps.length, 4);
  assert.deepEqual(reply.steps.at(-1), { tag: "Stopped", child: null, owner: null, action: "Retired", released: 11 });
});

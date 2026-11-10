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

const source = fileURLToPath(new URL("../../.improve/state-machine-checks/restart_state_bridge.rs", import.meta.url));
const work = mkdtempSync(join(tmpdir(), "arbos-restart-laws-"));
const environment = { PATH: process.env.PATH ?? "/usr/bin:/bin" };
const binary = join(work, process.platform === "win32" ? "restart-bridge.exe" : "restart-bridge");
const build = spawnSync("rustc", ["--edition=2021", "--crate-name", "restart_state_bridge", "-O", source, "-o", binary],
  { cwd: work, env: environment, encoding: "utf8", timeout: 30000 });
assert.equal(build.status, 0, "Unable to compile the actual pure restart reducer bridge");
const snapshot = Schema.Struct({ tag: Schema.Literals(["Serving", "Planned", "Shutdown"]),
  generation: Schema.optional(Schema.Int), phase: Schema.optional(Schema.Literals(["Freezing", "Notifying", "Draining", "Ready"])) });
const Reply = Schema.Struct({ id: Schema.Int.check(Schema.isBetween({ minimum: 0, maximum: 4294967295 })),
  steps: Schema.Array(snapshot) });
type Reply = typeof Reply.Type;
const decode = Schema.decodeUnknownSync(Reply, { onExcessProperty: "error" });

/** One persistent exact-SQL child; every request starts from an explicit actual state. */
class RestartBridge {
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
      if (this.failed || this.closing) throw new Error("Restart state oracle unavailable");
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
    this.failed ??= new Error("Restart state oracle failed");
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

const bridge = new RestartBridge();
after(async () => { await bridge.close(); rmSync(work, { recursive: true, force: true }); });
const input = Schema.Struct({ tail: Schema.Array(Schema.Boolean).check(Schema.isMaxLength(24)),
  wrongOwner: Schema.Int.check(Schema.isBetween({ minimum: 2, maximum: 20 })),
  phase: Schema.Literals(["n", "d", "r"]) });
type Input = typeof input.Type;
const canonical: Input = { tail: [], wrongOwner: 2, phase: "n" };
const arbitrary = Arbitrary.schema(input, { shrink: (value) => isDeepStrictEqual(value, canonical) ? [] : [canonical] });
function law(name: string, seed: number, predicate: (value: Input) => Promise<boolean>) {
  test(`${name}: 256 seeded traces`, async () => {
    const property = (value: Input) => Effect.tryPromise({ try: () => predicate(value), catch: () => new Error("Restart state observation failed") });
    const result = await Effect.runPromise(Arbitrary.checkEffect(arbitrary, property,
      { runs: 256, seed, size: 64, maxShrinks: 256, concurrency: 1 }));
    if (result._tag === "Falsified") {
      const replay = await Effect.runPromise(Arbitrary.checkEffect(arbitrary, property, { replay: result.replay }));
      assert.equal(replay._tag, "Falsified");
      if (replay._tag === "Falsified") assert.deepEqual(replay.shrunkInput, result.shrunkInput);
      console.log("restart-counterexample:" + Buffer.from(JSON.stringify({ law: name, seed, result, replay })).toString("base64"));
    }
    assert.equal(result._tag, "Passed", Arbitrary.formatCheckFailure(result));
    if (result._tag === "Passed") assert.equal(result.runs, 256);
  });
}

law("legal planned restart order retains one reset owner until release", 82001, async (value) => {
  const reply = await bridge.request(["p", "a:1:n", "a:1:d", "a:1:r", ...value.tail.map((flag) => flag ? "p" : "a:1:n"), "r:1", "p"]);
  return reply.steps[0].tag === "Planned" && reply.steps[0].phase === "Freezing"
    && reply.steps[1].phase === "Notifying" && reply.steps[2].phase === "Draining"
    && reply.steps.slice(3, -2).every((state) => isDeepStrictEqual(state, { tag: "Planned", generation: 1, phase: "Ready" }))
    && isDeepStrictEqual(reply.steps.at(-2), { tag: "Serving", generation: 1 })
    && isDeepStrictEqual(reply.steps.at(-1), { tag: "Planned", generation: 2, phase: "Freezing" });
});

law("stale planned cleanup and out-of-order progress cannot change replacement ownership", 82002, async (value) => {
  const before = ["p", "r:1", "p", ...value.tail.map((flag) => flag ? "r:1" : "a:1:d"), "r:1", "p", "a:2:r"];
  const reply = await bridge.request([...before, "a:2:n", "a:2:d", "a:2:r", "r:2"]);
  return isDeepStrictEqual(reply.steps[1], { tag: "Serving", generation: 1 })
    && reply.steps.slice(2, before.length).every((state) => isDeepStrictEqual(state, { tag: "Planned", generation: 2, phase: "Freezing" }))
    && isDeepStrictEqual(reply.steps.at(-1), { tag: "Serving", generation: 2 });
});

law("shutdown absorbs every later planned lease event", 82003, async (value) => {
  const reply = await bridge.request(["p", "s", ...value.tail.map((flag) => flag ? "p" : "r:1"), "a:1:n", "r:1", "p"]);
  return reply.steps[0].tag === "Planned" && reply.steps.slice(1).every((state) => isDeepStrictEqual(state, { tag: "Shutdown" }));
});

law("a different owner cannot advance an admitted attempt", 82004, async (value) => {
  const wrong = [`a:${value.wrongOwner}:${value.phase}`, `r:${value.wrongOwner}`];
  const reply = await bridge.request(["p", ...wrong, "a:1:n", "a:1:d", "a:1:r", "r:1"]);
  return reply.steps.slice(0, 3).every((state) => isDeepStrictEqual(state, { tag: "Planned", generation: 1, phase: "Freezing" }))
    && isDeepStrictEqual(reply.steps.at(-1), { tag: "Serving", generation: 1 });
});

test("bounded native reducer protocol rejects malformed traces and recovers", () => {
  const bad = ["broken", "1|unknown", "1|a:1:bad", "1|" + Array(65).fill("p").join(","), "x".repeat(8193)];
  const child = spawnSync(binary, [], { cwd: work, env: environment,
    input: [...bad, "9|p,a:1:n,a:1:d,a:1:r,r:1"].join("\n") + "\n", encoding: "utf8", timeout: 5000 });
  assert.equal(child.status, 0); assert.equal(child.stderr, "");
  const replies = child.stdout.trim().split("\n").map((line) => JSON.parse(line));
  assert.equal(replies.length, bad.length + 1);
  assert.ok(replies.slice(0, -1).every((reply) => reply.id === null && reply.error === "Invalid trace" && Object.keys(reply).length === 2));
  const reply = decode(replies.at(-1)); assert.equal(reply.id, 9); assert.equal(reply.steps.length, 5);
  assert.deepEqual(reply.steps.at(-1), { tag: "Serving", generation: 1 });
});

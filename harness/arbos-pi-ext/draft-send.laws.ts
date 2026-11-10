import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, readFileSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { after, test } from "node:test";
import { fileURLToPath } from "node:url";
import { isDeepStrictEqual } from "node:util";
import * as Arbitrary from "effect/Arbitrary";
import * as Effect from "effect/Effect";
import * as Schema from "effect/Schema";

const script = fileURLToPath(new URL("../../.improve/state-machine-checks/ios-account-work/draft_bridge.py", import.meta.url));
const cache = mkdtempSync("/tmp/arbos-draft-laws-");
const environment = { PATH: process.env.PATH ?? "/usr/bin:/bin" };
const prepared = spawnSync("python3", ["-I", "-B", script, "--prepare", "--cache", cache],
  { env: environment, encoding: "utf8", timeout: 150_000, maxBuffer: 65_536 });
assert.equal(prepared.status, 0, "Native oracle setup failed; inspect compiler logs in the owned scratch directory");
const setup = JSON.parse(prepared.stdout) as { compiled: boolean; sources: Record<string, string> };
assert.equal(setup.compiled, true);
const sourceHashes = () => Object.fromEntries(Object.keys(setup.sources).map((path) =>
  [path, createHash("sha256").update(readFileSync(new URL("../../" + path, import.meta.url))).digest("hex")]));
const outcomes: { seed: number; outcome: string; runs: number }[] = [];

type Action = { kind: "enqueue"; account: number; draft: string; value: number }
  | { kind: "finish"; index: number } | { kind: "retire" } | { kind: "fresh"; account: number };
const Observation = Schema.Struct({ outcome: Schema.Literals(["none", "started", "queued", "refused", "finished", "retired", "fresh"]),
  issued: Schema.NullOr(Schema.Int), sends: Schema.NullOr(Schema.Array(Schema.Int)), pending: Schema.Array(Schema.Boolean), owner: Schema.NullOr(Schema.Int) });
const Reply = Schema.Struct({ id: Schema.Int, steps: Schema.Array(Observation) });
type Reply = typeof Reply.Type;
const decode = Schema.decodeUnknownSync(Reply, { onExcessProperty: "error" });

class Bridge {
  private readonly child;
  private readonly exited: Promise<void>;
  private sequence = 0;
  private buffer = "";
  private closing = false;
  private failed = false;
  private tail: Promise<unknown> = Promise.resolve();
  private pending: { id: number; count: number | undefined; resolve: (reply: Reply) => void; reject: (error: Error) => void; timer: NodeJS.Timeout } | undefined;
  constructor() {
    this.child = spawn("python3", ["-I", "-B", script, "--cache", cache, "--language", "swift"], { cwd: cache, env: environment, stdio: "pipe" });
    this.exited = new Promise((resolve) => {
      this.child.once("close", () => { if (!this.closing) this.fail(); resolve(); });
      this.child.once("error", () => { this.fail(); resolve(); });
    });
    this.child.stdin.on("error", () => this.fail());
    this.child.stderr.on("data", () => {});
    this.child.stdout.setEncoding("utf8");
    this.child.stdout.on("data", (chunk: string) => {
      this.buffer += chunk;
      if (Buffer.byteLength(this.buffer) > 262_144) { this.fail(); return; }
      for (;;) {
        const end = this.buffer.indexOf("\n");
        if (end < 0) return;
        const line = this.buffer.slice(0, end); this.buffer = this.buffer.slice(end + 1);
        try {
          const reply = decode(JSON.parse(line));
          const pending = this.pending;
          if (!pending || reply.id !== pending.id || (pending.count !== undefined && reply.steps?.length !== pending.count)
              ) throw new Error("Invalid observation");
          clearTimeout(pending.timer); this.pending = undefined; pending.resolve(reply);
        } catch { this.fail(); return; }
      }
    });
  }
  request(body: { actions: readonly Action[] }): Promise<Reply> {
    const next = this.tail.then(() => {
      if (this.failed || this.closing) throw new Error("Native observation unavailable");
      const id = this.sequence++;
      const line = JSON.stringify({ id, ...body });
      if ((body.actions?.length ?? 0) > 64 || Buffer.byteLength(line) > 65_536) throw new Error("Trace exceeds bound");
      return new Promise<Reply>((resolve, reject) => {
        this.pending = { id, count: body.actions?.length, resolve, reject, timer: setTimeout(() => this.fail(), 5000) };
        this.child.stdin.write(line + "\n");
      });
    });
    this.tail = next.catch(() => undefined);
    return next;
  }
  private fail(): void {
    this.failed = true;
    if (this.pending) { clearTimeout(this.pending.timer); this.pending.reject(new Error("Native observation failed")); this.pending = undefined; }
    this.child.kill();
  }
  async close(): Promise<void> {
    this.closing = true; this.child.stdin.end();
    const timer = setTimeout(() => this.child.kill("SIGKILL"), 1000);
    try { await this.exited; } finally { clearTimeout(timer); }
  }
}
const bridge = new Bridge();
after(async () => {
  await bridge.close();
  assert.deepEqual(sourceHashes(), setup.sources, "Native sources changed during the law run");
  if (process.env.ARBOS_DRAFT_RECORD) writeFileSync(process.env.ARBOS_DRAFT_RECORD, JSON.stringify({ sources: setup.sources, outcomes }, null, 2) + "\n");
  rmSync(cache, { recursive: true, force: true });
});

const Input = Schema.Struct({
  a: Schema.Array(Schema.Int.check(Schema.isBetween({ minimum: -100, maximum: 100 }))).check(Schema.isBetweenLength(1, 8)),
  b: Schema.Array(Schema.Int.check(Schema.isBetween({ minimum: -100, maximum: 100 }))).check(Schema.isBetweenLength(1, 8)),
  noise: Schema.Array(Schema.Literals(["finish", "retire"])).check(Schema.isMaxLength(8)),
});
type Input = typeof Input.Type;
const canonical: Input = { a: [0], b: [1], noise: [] };
const arbitrary = Arbitrary.schema(Input, { shrink: (value) => isDeepStrictEqual(value, canonical) ? [] : [canonical] });
const enqueue = (draft: string, value: number, account = 0): Action => ({ kind: "enqueue", draft, value, account });
const finish = (index: number): Action => ({ kind: "finish", index });
function law(name: string, seed: number, predicate: (value: Input) => Promise<boolean>) {
  if (process.env.ARBOS_DRAFT_LAW && process.env.ARBOS_DRAFT_LAW !== String(seed)) return;
  test(`${name}: 256 seeded cases`, async () => {
    if (process.env.ARBOS_DRAFT_REPLAY) {
      const value = Schema.decodeUnknownSync(Input, { onExcessProperty: "error" })(JSON.parse(process.env.ARBOS_DRAFT_REPLAY));
      assert.equal(await predicate(value), true, `seed=${seed} replay=${JSON.stringify(value)}`); return;
    }
    const result = await Effect.runPromise(Arbitrary.checkEffect(arbitrary,
      (value) => Effect.tryPromise({ try: () => predicate(value), catch: () => new Error("Draft observation failed") }),
      { runs: 256, seed, size: 32, maxShrinks: 256, concurrency: 1 }));
    if (process.env.ARBOS_DRAFT_RESULT) writeFileSync(process.env.ARBOS_DRAFT_RESULT, JSON.stringify(result, null, 2) + "\n");
    outcomes.push({ seed, outcome: result._tag, runs: result._tag === "Passed" ? result.runs : 0 });
    assert.equal(result._tag, "Passed", result._tag === "Passed" ? "" : Arbitrary.formatCheckFailure(result));
    if (result._tag === "Passed") assert.equal(result.runs, 256);
  });
}
law("draft callbacks coalesce FIFO only within their own target", 90008, async ({ a, b }) => {
  const steps = (await bridge.request({ actions: [...a.map((value) => enqueue("a", value)), ...b.map((value) => enqueue("b", value)), finish(a.length), finish(0), finish(0)] })).steps;
  return steps[0]?.outcome === "started" && steps[a.length]?.outcome === "started"
    && steps.slice(1, a.length).every((step) => step.outcome === "queued")
    && steps.slice(a.length + 1, a.length + b.length).every((step) => step.outcome === "queued")
    && isDeepStrictEqual(steps.at(-3)?.sends, b) && isDeepStrictEqual(steps.at(-2)?.sends, a)
    && isDeepStrictEqual(steps.at(-3)?.pending, [true, false, false])
    && isDeepStrictEqual(steps.at(-2)?.pending, [false, false, false]) && steps.at(-1)?.sends === null;
});
law("old completion cannot take a same-account replacement operation", 90009, async ({ a, b, noise }) => {
  const distractions: Action[] = noise.map((kind) => kind === "retire" ? { kind } : finish(0));
  const steps = (await bridge.request({ actions: [enqueue("a", a[0]!), ...distractions, { kind: "retire" }, { kind: "fresh", account: 0 }, ...b.map((value) => enqueue("a", value)), finish(0), finish(1)] })).steps;
  return steps[0]?.outcome === "started" && steps.at(-2)?.sends === null
    && steps.at(-2)?.pending[0] === true && isDeepStrictEqual(steps.at(-1)?.sends, b) && steps.at(-1)?.pending[0] === false;
});
law("retired queue absorbs late work and current queue rejects foreign authority", 90010, async ({ a, b, noise }) => {
  const retiredNoise: Action[] = noise.map((kind) => kind === "retire" ? { kind } : finish(0));
  const steps = (await bridge.request({ actions: [enqueue("a", a[0]!), enqueue("b", b[0]!, 1), enqueue("", b[0]!), { kind: "retire" }, ...retiredNoise,
    enqueue("a", b[0]!), finish(0), { kind: "fresh", account: 1 }, enqueue("b", b[0]!, 1), finish(1)] })).steps;
  return steps[0]?.outcome === "started" && steps[1]?.outcome === "refused" && steps[2]?.outcome === "refused"
    && steps.slice(3, -3).every((step) => step.owner === null && isDeepStrictEqual(step.pending, [false, false, false]) && step.sends === null)
    && steps.at(-2)?.outcome === "started" && isDeepStrictEqual(steps.at(-1)?.sends, [b[0]!]);
});
test("draft observation boundary rejects bad frames and recovers", () => {
  const good = { id: 5, actions: [enqueue("a", 7)] };
  const bad = ["{", JSON.stringify({ id: 0, actions: [], extra: true }), JSON.stringify({ id: 1, actions: [{ kind: "execute" }] }),
    JSON.stringify({ id: 2, actions: [enqueue("a", 1, 2)] }), JSON.stringify({ id: 3, actions: Array(65).fill({ kind: "retire" }) }), "x".repeat(65_537)];
  const child = spawnSync("python3", ["-I", "-B", script, "--cache", cache, "--language", "swift"],
    { env: environment, input: [...bad, JSON.stringify(good)].join("\n") + "\n", encoding: "utf8", timeout: 5000 });
  assert.equal(child.status, 0); assert.equal(child.stderr, "");
  const replies = child.stdout.trim().split("\n").map((line) => JSON.parse(line));
  assert.equal(replies.length, bad.length + 1);
  assert.ok(replies.slice(0, -1).every((reply) => reply.error === "Invalid trace"));
  assert.equal(decode(replies.at(-1)).steps[0]?.outcome, "started");
});

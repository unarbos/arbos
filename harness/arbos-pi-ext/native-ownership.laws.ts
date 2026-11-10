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

const script = fileURLToPath(new URL("../../.improve/state-machine-checks/native-ownership/bridge.py", import.meta.url));
const cache = mkdtempSync("/tmp/arbos-native-laws-");
const environment = { PATH: process.env.PATH ?? "/usr/bin:/bin" };
const prepared = spawnSync("python3", ["-I", "-B", script, "--prepare", "--cache", cache],
  { env: environment, encoding: "utf8", timeout: 250_000, maxBuffer: 65_536 });
assert.equal(prepared.status, 0, "Native oracle setup failed; inspect compiler logs in the owned scratch directory");
const setup = JSON.parse(prepared.stdout) as { compiled: boolean; sources: Record<string, string> };
assert.equal(setup.compiled, true);
const sourceHashes = () => Object.fromEntries(Object.keys(setup.sources).map((path) =>
  [path, createHash("sha256").update(readFileSync(new URL("../../" + path, import.meta.url))).digest("hex")]));
const outcomes: { seed: number; outcome: string; runs: number }[] = [];

type Language = "swift" | "rust";
type Action = { kind: string; index?: number };
const Observation = Schema.Struct({ accepted: Schema.Array(Schema.Boolean), issued: Schema.NullOr(Schema.Int), hasCurrent: Schema.Boolean,
  other: Schema.optional(Schema.Boolean) });
const Reply = Schema.Struct({ id: Schema.Int, initial: Schema.optional(Observation), steps: Schema.optional(Schema.Array(Observation)),
  accepted: Schema.optional(Schema.Boolean) });
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
  constructor(language: Language) {
    this.child = spawn("python3", ["-I", "-B", script, "--cache", cache, "--language", language], { cwd: cache, env: environment, stdio: "pipe" });
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
              || (pending.count === undefined && typeof reply.accepted !== "boolean")) throw new Error("Invalid observation");
          clearTimeout(pending.timer); this.pending = undefined; pending.resolve(reply);
        } catch { this.fail(); return; }
      }
    });
  }
  request(body: { machine: string; actions?: readonly Action[]; snapshot?: Snapshot }): Promise<Reply> {
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
const bridges = { swift: new Bridge("swift"), rust: new Bridge("rust") };
after(async () => {
  await Promise.all([bridges.swift.close(), bridges.rust.close()]);
  assert.deepEqual(sourceHashes(), setup.sources, "Native sources changed during the law run");
  if (process.env.ARBOS_NATIVE_RECORD) writeFileSync(process.env.ARBOS_NATIVE_RECORD, JSON.stringify({ sources: setup.sources, outcomes }, null, 2) + "\n");
  rmSync(cache, { recursive: true, force: true });
});

const Input = Schema.Struct({
  noise: Schema.Array(Schema.Literals(["capture", "reopen", "close"])).check(Schema.isMaxLength(8)),
  late: Schema.Array(Schema.Literals(["ready", "failed"])).check(Schema.isMaxLength(8)),
  closed: Schema.Boolean, foreign: Schema.Boolean, disposed: Schema.Boolean, sameDoc: Schema.Boolean, empty: Schema.Boolean,
  revision: Schema.Int.check(Schema.isBetween({ minimum: 0, maximum: 8 })),
  latest: Schema.Int.check(Schema.isBetween({ minimum: 0, maximum: 8 })),
});
type Input = typeof Input.Type;
type Snapshot = Pick<Input, "closed" | "foreign" | "disposed" | "sameDoc" | "empty" | "revision" | "latest">;
const canonical: Input = { noise: [], late: [], closed: true, foreign: false, disposed: false, sameDoc: true, empty: true, revision: 0, latest: 1 };
const arbitrary = Arbitrary.schema(Input, { shrink: (value) => isDeepStrictEqual(value, canonical) ? [] : [canonical] });
const action = (kind: string, index?: number): Action => index === undefined ? { kind } : { kind, index };

function law(name: string, seed: number, predicate: (value: Input) => Promise<boolean>) {
  if (process.env.ARBOS_NATIVE_LAW && process.env.ARBOS_NATIVE_LAW !== String(seed)) return;
  test(`${name}: 256 seeded cases`, async () => {
    if (process.env.ARBOS_NATIVE_REPLAY) {
      const value = Schema.decodeUnknownSync(Input, { onExcessProperty: "error" })(JSON.parse(process.env.ARBOS_NATIVE_REPLAY));
      assert.equal(await predicate(value), true, `seed=${seed} replay=${JSON.stringify(value)}`); return;
    }
    const result = await Effect.runPromise(Arbitrary.checkEffect(arbitrary,
      (value) => Effect.tryPromise({ try: () => predicate(value), catch: () => new Error("Native observation failed") }),
      { runs: 256, seed, size: 32, maxShrinks: 256, concurrency: 1 }));
    outcomes.push({ seed, outcome: result._tag, runs: result._tag === "Passed" ? result.runs : 0 });
    assert.equal(result._tag, "Passed", result._tag === "Passed" ? "" : Arbitrary.formatCheckFailure(result));
    if (result._tag === "Passed") assert.equal(result.runs, 256);
  });
}
for (const [language, workSeed, matrixSeed] of [["swift", 90001, 90002], ["rust", 90004, 90005]] as const) {
  law(`${language} lifetime never revives an old or foreign ticket`, workSeed, async (value) => {
    const actions = [action("close"), ...value.noise.map((kind) => action(kind)), action("reopen")];
    const reply = await bridges[language].request({ machine: "work", actions });
    const initial = reply.initial;
    const steps = reply.steps;
    return initial?.accepted[0] === true && initial.other === false && initial.hasCurrent
      && steps?.[0]?.accepted[0] === false && steps[0]?.hasCurrent === false
      && steps.every((step) => step.accepted[0] === false && step.other === false)
      && steps.at(-1)?.hasCurrent === true && steps.at(-1)?.accepted.at(-1) === true;
  });
  law(`${language} old Matrix completions cannot retire or replace a newer attempt`, matrixSeed, async (value) => {
    const actions = [action("start"), action("ready", 0), action("stop"), action("start"),
      ...value.late.map((kind) => action(kind, 0)), action("ready", 1), action("failed", 0), action("start")];
    const reply = await bridges[language].request({ machine: "matrix", actions });
    const steps = reply.steps;
    return steps?.[0]?.issued === 0 && steps[0]?.accepted[0] === true
      && steps[2]?.accepted[0] === false && steps[3]?.issued === 1
      && steps.slice(3).every((step) => isDeepStrictEqual(step.accepted, [false, true]))
      && steps.at(-1)?.issued === null;
  });
}
law("Swift snapshot rejects disposed owners/docs and preserves same-doc first paint", 90003, async (value) => {
  const snapshot: Snapshot = { closed: value.closed, foreign: value.foreign, disposed: value.disposed,
    sameDoc: value.sameDoc, empty: value.empty, revision: value.revision, latest: value.latest };
  const reply = await bridges.swift.request({ machine: "snapshot", snapshot });
  const expected = !value.closed && !value.foreign && !value.disposed && value.sameDoc
    && (value.empty || value.revision === value.latest);
  const current = { ...snapshot, closed: false, foreign: false, disposed: false, sameDoc: true, empty: false, latest: value.revision };
  const firstPaint = { ...current, empty: true, latest: value.revision + 1 };
  return reply.accepted === expected
    && (await bridges.swift.request({ machine: "snapshot", snapshot: current })).accepted === true
    && (await bridges.swift.request({ machine: "snapshot", snapshot: firstPaint })).accepted === true;
});

law("Rust UI session rejects stale controls/data while current adoption progresses", 90006, async (value) => {
  const noise = value.noise.map((kind) => action(kind === "capture" ? "adopt_a" : kind === "reopen" ? "renew" : "clear"));
  const actions = [action("adopt_a"), action("adopt_b"), ...noise, action("clear"), action("adopt_b"), action("adopt_old"), action("adopt_foreign"), action("unbind"), action("adopt_b")];
  const reply = await bridges.rust.request({ machine: "ui", actions });
  const steps = reply.steps!;
  const tail = steps.slice(2 + noise.length);
  return reply.initial?.hasCurrent === true && isDeepStrictEqual(steps[0]?.accepted, [true, false, true, false]) && steps[0]?.other === true
    && isDeepStrictEqual(steps[1]?.accepted, [false, true, false, true]) && steps[1]?.hasCurrent === true
    && isDeepStrictEqual(tail[0]?.accepted, [false, false, false, false]) && tail[0]?.hasCurrent === false
    && isDeepStrictEqual(tail[1]?.accepted, [false, true, false, true]) && tail[1]?.other === true
    && isDeepStrictEqual(tail[2]?.accepted, [false, true, false, true]) && tail[2]?.other === false
    && isDeepStrictEqual(tail[3]?.accepted, [false, true, false, true]) && tail[3]?.other === false
    && isDeepStrictEqual(tail[4]?.accepted, [false, false, false, false])
    && isDeepStrictEqual(tail[5]?.accepted, [false, true, false, true]) && tail[5]?.other === true;
});
law("Rust retired task owner rejects late effects and repeat retirement is empty", 90007, async (value) => {
  const noise = value.noise.map((kind) => action(kind === "capture" ? "capture" : "close"));
  const actions = [action("capture"), ...noise, action("close"), action("capture"), action("capture"), action("close"), action("capture")];
  const reply = await bridges.rust.request({ machine: "tasks", actions });
  const tail = reply.steps!.slice(1 + noise.length);
  return isDeepStrictEqual(reply.steps?.[0]?.accepted, [true, true])
    && isDeepStrictEqual(tail[1]?.accepted, [false, false]) && isDeepStrictEqual(tail[2]?.accepted, [false, false])
    && tail[3]?.issued === 0 && isDeepStrictEqual(tail[4]?.accepted, [false, false]);
});

test("native observation boundary rejects bad frames and recovers", () => {
  const good = { id: 5, machine: "work", actions: [{ kind: "close" }] };
  for (const language of ["swift", "rust"]) {
    const bad = ["{", JSON.stringify({ id: 0, machine: "work", actions: [], extra: true }),
      JSON.stringify({ id: 1, machine: "work", actions: [{ kind: "execute" }] }),
      JSON.stringify({ id: 2, machine: "work", actions: Array(65).fill({ kind: "close" }) }), "x".repeat(65_537)];
    const child = spawnSync("python3", ["-I", "-B", script, "--cache", cache, "--language", language],
      { env: environment, input: [...bad, JSON.stringify(good)].join("\n") + "\n", encoding: "utf8", timeout: 5000 });
    assert.equal(child.status, 0); assert.equal(child.stderr, "");
    const replies = child.stdout.trim().split("\n").map((line) => JSON.parse(line));
    assert.equal(replies.length, bad.length + 1);
    assert.ok(replies.slice(0, -1).every((reply) => reply.error === "Invalid trace"));
    assert.equal(decode(replies.at(-1)).steps?.[0]?.accepted[0], false);
  }
});

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

const script = fileURLToPath(new URL("../../scripts/feedback/feedback_state_bridge.py", import.meta.url));
const work = mkdtempSync(join(tmpdir(), "arbos-feedback-laws-"));
const environment = { PATH: process.env.PATH ?? "/usr/bin:/bin" };
const args = ["-I", "-B", script];
type Machine = "commit" | "delivery";
type State = Readonly<Record<string, Schema.Json>>;
type Event = State;
const snapshot = Schema.Struct({
  _tag: Schema.Literals(["Candidate", "Held", "AwaitingApproval", "GatePending", "MissingHead", "StaleHead", "ReadyToMerge",
    "Landed", "InvalidControl", "DeliveryPending", "AwaitingArtifacts", "AwaitingDeploy", "Shipped"]),
  head: Schema.optional(Schema.String), live: Schema.optional(Schema.String), merge: Schema.optional(Schema.String),
  reason: Schema.optional(Schema.String), commit: Schema.optional(Schema.String), device_evidence: Schema.optional(Schema.String),
  pending_deploy: Schema.optional(Schema.Array(Schema.String).check(Schema.isMaxLength(3))),
});
const Reply = Schema.Struct({
  id: Schema.Int.check(Schema.isBetween({ minimum: 0, maximum: Number.MAX_SAFE_INTEGER })),
  initial: snapshot, steps: Schema.Array(snapshot),
});
type Reply = typeof Reply.Type;
const decode = Schema.decodeUnknownSync(Reply, { onExcessProperty: "error" });

/** One persistent pure-transition child; every request starts from an explicit actual state. */
class FeedbackBridge {
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

  request(machine: Machine, state: State, events: readonly Event[]): Promise<Reply> {
    const next = this.tail.then(() => {
      if (this.failed || this.closing) throw new Error("Pure feedback oracle unavailable");
      const id = this.nextId++;
      const line = JSON.stringify({ id, machine, state, events });
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
    this.failed ??= new Error("Pure feedback oracle failed");
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





const bridge = new FeedbackBridge();
after(async () => { await bridge.close(); rmSync(work, { recursive: true, force: true }); });
const head = "a".repeat(40), other = "b".repeat(40), carrier = "c".repeat(40);
const input = Schema.Struct({
  held: Schema.Boolean, approval: Schema.Boolean, green: Schema.Boolean, eligible: Schema.Boolean,
  live: Schema.Literals(["same", "other", "missing"]), covered: Schema.Literals(["same", "other", "missing"]),
  deploy: Schema.Literals(["voice", "relay", "both", "none"]),
  evidence: Schema.Literals(["pending", "not needed", "confirmed on synthetic device"]),
  tail: Schema.Array(Schema.Boolean).check(Schema.isMaxLength(24)),
});
type Input = typeof input.Type;
const canonical: Input = { held: false, approval: false, green: false, eligible: false,
  live: "same", covered: "same", deploy: "none", evidence: "not needed", tail: [] };
const arbitrary = Arbitrary.schema(input, { shrink: (value) => isDeepStrictEqual(value, canonical) ? [] : [canonical] });
const observe = (extra: Event = {}): Event => ({ _tag: "ObserveGate", covered_head: head, live_head: head,
  green: true, eligible: true, held: false, approval_required: false, ...extra });
const kinds = (value: Input) => value.deploy === "none" ? [] : value.deploy === "both" ? ["voice", "relay"] : [value.deploy];
const delivery = (value: Input, waiting: boolean, pending = kinds(value)): Event => ({ _tag: "ObserveDelivery",
  pending_deploy: pending, waiting_ios: waiting, device_evidence: value.evidence });

function law(name: string, seed: number, predicate: (value: Input) => Promise<boolean>) {
  test(`${name}: 256 seeded traces`, async () => {
    const property = (value: Input) => Effect.tryPromise({ try: () => predicate(value),
      catch: () => new Error("Feedback state observation failed") });
    const result = await Effect.runPromise(Arbitrary.checkEffect(arbitrary, property,
      { runs: 256, seed, size: 64, maxShrinks: 256, concurrency: 1 }));
    if (result._tag === "Falsified") {
      const replay = await Effect.runPromise(Arbitrary.checkEffect(arbitrary, property, { replay: result.replay }));
      assert.equal(replay._tag, "Falsified");
      if (replay._tag === "Falsified") assert.deepEqual(replay.shrunkInput, result.shrunkInput);
      console.log("feedback-counterexample:" + Buffer.from(JSON.stringify({ law: name, seed, result, replay })).toString("base64"));
    }
    assert.equal(result._tag, "Passed", Arbitrary.formatCheckFailure(result));
    if (result._tag === "Passed") assert.equal(result.runs, 256);
  });
}

law("commit admission requires approval hold and exact live coverage", 84601, async (value) => {
  const selected = (choice: string): string | null => choice === "same" ? head : choice === "other" ? other : null;
  const reply = await bridge.request("commit", { _tag: "Candidate", head }, [observe({ held: value.held,
    approval_required: value.approval, green: value.green, eligible: value.eligible,
    covered_head: selected(value.covered), live_head: selected(value.live) })]);
  const allowed = !value.held && !value.approval && value.green && value.eligible && value.covered === "same" && value.live === "same";
  const positive = await bridge.request("commit", { _tag: "Candidate", head }, [observe()]);
  return (reply.steps[0]._tag === "ReadyToMerge") === allowed && positive.steps[0]._tag === "ReadyToMerge";
});

law("an unapproved or held commit cannot complete without fresh admission", 84602, async (value) => {
  for (const flag of ["approval_required", "held"]) {
    const reply = await bridge.request("commit", { _tag: "Candidate", head }, [observe({ [flag]: true }),
      { _tag: "Merged", sha: carrier }, ...value.tail.map((same) => ({ _tag: "Merged", sha: same ? head : other }))]);
    if (reply.steps.some((step) => step._tag === "ReadyToMerge" || step._tag === "Landed")) return false;
  }
  return true;
});

law("deployment completion cannot substitute for an installable iOS artifact", 84603, async (value) => {
  const reply = await bridge.request("delivery", { _tag: "DeliveryPending", commit: head }, [
    delivery(value, true), { _tag: "Deployed", commit: head },
    ...value.tail.map(() => ({ _tag: "Deployed", commit: head })), delivery(value, false, []),
  ]);
  return reply.steps.slice(0, -1).every((step) => step._tag === "AwaitingArtifacts" && step.device_evidence === value.evidence)
    && reply.steps.at(-1)?._tag === "Shipped" && reply.steps.at(-1)?.device_evidence === value.evidence;
});

law("deployment proof remains bound to its exact commit", 84604, async (value) => {
  const first = delivery(value, false, kinds(value).length ? kinds(value) : ["voice"]);
  const wrong = await bridge.request("delivery", { _tag: "DeliveryPending", commit: head }, [first, { _tag: "Deployed", commit: other }]);
  const right = await bridge.request("delivery", { _tag: "DeliveryPending", commit: head }, [first, { _tag: "Deployed", commit: head }]);
  return wrong.steps[0]._tag === "AwaitingDeploy" && wrong.steps[1]._tag === "InvalidControl"
    && right.steps[1]._tag === "Shipped" && right.steps[1].commit === head && right.steps[1].device_evidence === value.evidence;
});

law("landed and shipped phases absorb later lifecycle events", 84605, async (value) => {
  const committed = await bridge.request("commit", { _tag: "Candidate", head }, [observe(), { _tag: "Merged", sha: carrier },
    observe({ held: true }), ...value.tail.map((same) => ({ _tag: "Merged", sha: same ? head : other }))]);
  const shipped = await bridge.request("delivery", { _tag: "DeliveryPending", commit: head }, [delivery(value, false, []),
    delivery(value, true), ...value.tail.map((same) => ({ _tag: "Deployed", commit: same ? head : other }))]);
  return committed.steps[1]._tag === "Landed" && committed.steps.slice(1).every((step) => isDeepStrictEqual(step, committed.steps[1]))
    && shipped.steps[0]._tag === "Shipped" && shipped.steps.every((step) => isDeepStrictEqual(step, shipped.steps[0]));
});

test("pure feedback protocol rejects wrong shapes and recovers", () => {
  const valid = { id: 9, machine: "commit", state: { _tag: "Candidate", head }, events: [] };
  const bad = ["{", JSON.stringify({ ...valid, extra: true }), JSON.stringify({ ...valid, machine: "unknown" }),
    JSON.stringify({ ...valid, events: [{ _tag: "Merged", sha: true }] }),
    JSON.stringify({ ...valid, events: Array(65).fill({ _tag: "Merged", sha: head }) }), "x".repeat(65537)];
  const child = spawnSync("python3", args, { cwd: work, env: environment,
    input: [...bad, JSON.stringify(valid)].join("\n") + "\n", encoding: "utf8", timeout: 5000 });
  assert.equal(child.status, 0);
  assert.equal(child.stderr, "");
  const replies = child.stdout.trim().split("\n").map((line) => JSON.parse(line));
  assert.equal(replies.length, bad.length + 1);
  assert.ok(replies.slice(0, -1).every((reply) => reply.error === "Invalid trace" && Object.keys(reply).length === 2));
  assert.deepEqual(decode(replies.at(-1)), { id: 9, initial: { _tag: "Candidate", head }, steps: [] });
});

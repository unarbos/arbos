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

const script = fileURLToPath(new URL("../../.improve/state-machine-checks/once_claim_bridge.py", import.meta.url));
const work = mkdtempSync(join(tmpdir(), "arbos-once-claim-laws-"));
const environment = { PATH: process.env.PATH ?? "/usr/bin:/bin" };
const args = ["-I", "-B", script];
const GrantSchema = Schema.Struct({
  id: Schema.String, user_id: Schema.String, project_id: Schema.String, chat_id: Schema.String,
  target_kind: Schema.String, target_id: Schema.NullOr(Schema.String), target_ref: Schema.String,
  use_kind: Schema.String, host: Schema.NullOr(Schema.String), asked_by: Schema.NullOr(Schema.String),
  decided_by: Schema.NullOr(Schema.String), decided_at: Schema.NullOr(Schema.Int), expires_at: Schema.Int,
  status: Schema.Literals(["allowed_once", "expired", "pending", "denied"]),
});
type Grant = typeof GrantSchema.Type;
type Claim = Omit<Grant, "status">;
type Event = { claims: readonly Claim[]; now: number; fail_id: string | null };
const Reply = Schema.Struct({
  id: Schema.Int.check(Schema.isBetween({ minimum: 0, maximum: Number.MAX_SAFE_INTEGER })),
  steps: Schema.Array(Schema.Struct({ outcome: Schema.Literals(["Claimed", "Unavailable", "SqlError"]),
    rows: Schema.Array(Schema.Tuple([Schema.String, Schema.String])) })),
  sqlite: Schema.String, python: Schema.String, sql_sha256: Schema.String,
});
type Reply = typeof Reply.Type;
const decode = Schema.decodeUnknownSync(Reply, { onExcessProperty: "error" });

/** One persistent exact-SQL child; every request starts from an explicit actual state. */
class ClaimBridge {
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

  request(grants: readonly Grant[], events: readonly Event[]): Promise<Reply> {
    const next = this.tail.then(() => {
      if (this.failed || this.closing) throw new Error("Claim SQL oracle unavailable");
      const id = this.nextId++;
      const line = JSON.stringify({ id, grants, events });
      if (events.length > 32 || Buffer.byteLength(line) > 65536) throw new Error("Trace exceeds protocol bound");
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
    this.failed ??= new Error("Claim SQL oracle failed");
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

const bridge = new ClaimBridge();
after(async () => { await bridge.close(); rmSync(work, { recursive: true, force: true }); });
const fields = ["id", "user_id", "project_id", "chat_id", "target_kind", "target_id", "target_ref",
  "use_kind", "host", "asked_by", "decided_by", "decided_at", "expires_at"] as const;
const input = Schema.Struct({
  field: Schema.Literals(fields), mismatch: Schema.Boolean, nullable: Schema.Boolean,
  status: Schema.Literals(["allowed_once", "expired", "pending", "denied"]),
  time: Schema.Literals([999, 1000, 1001]), sqlError: Schema.Boolean,
  refs: Schema.Array(Schema.Literals([0, 1, 2])).check(Schema.isMaxLength(8)),
  tail: Schema.Array(Schema.Boolean).check(Schema.isMaxLength(8)),
});
type Input = typeof input.Type;
const canonical: Input = { field: "id", mismatch: false, nullable: false, status: "allowed_once",
  time: 1000, sqlError: false, refs: [], tail: [] };
const arbitrary = Arbitrary.schema(input, { shrink: (value) => isDeepStrictEqual(value, canonical) ? [] : [canonical] });
function grant(id: number, nullable = false, status: Grant["status"] = "allowed_once"): Grant {
  return { id: `r${id}`, user_id: "user", project_id: "project", chat_id: "chat", target_kind: "secret",
    target_id: nullable ? null : "secret", target_ref: "TOKEN/value", use_kind: "header",
    host: nullable ? null : "fixture.invalid", asked_by: nullable ? null : "asker",
    decided_by: nullable ? null : "owner", decided_at: nullable ? null : 100, expires_at: 1000, status };
}
function claim(value: Grant): Claim { const { status, ...snapshot } = value; return snapshot; }
const event = (claims: readonly Claim[], now = 1000, fail_id: string | null = null): Event => ({ claims, now, fail_id });
const rows = (grants: readonly Grant[]) => grants.map((value) => [value.id, value.status]);
function law(name: string, seed: number, predicate: (value: Input) => Promise<boolean>) {
  test(`${name}: 256 seeded traces`, async () => {
    const property = (value: Input) => Effect.tryPromise({ try: () => predicate(value), catch: () => new Error("Claim SQL observation failed") });
    const result = await Effect.runPromise(Arbitrary.checkEffect(arbitrary, property,
      { runs: 256, seed, size: 64, maxShrinks: 256, concurrency: 1 }));
    if (result._tag === "Falsified") {
      const replay = await Effect.runPromise(Arbitrary.checkEffect(arbitrary, property, { replay: result.replay }));
      assert.equal(replay._tag, "Falsified");
      if (replay._tag === "Falsified") assert.deepEqual(replay.shrunkInput, result.shrunkInput);
      console.log("once-claim-counterexample:" + Buffer.from(JSON.stringify({ law: name, seed, result, replay })).toString("base64"));
    }
    assert.equal(result._tag, "Passed", Arbitrary.formatCheckFailure(result));
    if (result._tag === "Passed") assert.equal(result.runs, 256);
  });
}

law("spent authorization absorbs later claim attempts", 81001, async (value) => {
  const stored = grant(0, value.nullable);
  const reply = await bridge.request([stored], [event([claim(stored)]), event([claim(stored)]),
    ...value.tail.map(() => event([claim(stored)], value.time))]);
  return reply.steps[0].outcome === "Claimed" && reply.steps.slice(1).every((step) => step.outcome === "Unavailable")
    && reply.steps.every((step) => isDeepStrictEqual(step.rows, [[stored.id, "expired"]]));
});

law("admission requires exact observed scope live status and current expiry", 81002, async (value) => {
  const stored = grant(0, value.nullable, value.status);
  const snapshot = claim(stored);
  const changed = { ...snapshot, ...(value.mismatch ? { [value.field]: value.field === "decided_at" || value.field === "expires_at"
    ? (snapshot[value.field] ?? 100) + 1 : "changed" } : {}) };
  const reply = await bridge.request([stored], [event([changed], value.time)]);
  const expected = value.status === "allowed_once" && value.time <= stored.expires_at && !value.mismatch;
  const positive = grant(1, value.nullable);
  const control = await bridge.request([positive], [event([claim(positive)], positive.expires_at)]);
  return (reply.steps[0].outcome === "Claimed") === expected
    && isDeepStrictEqual(reply.steps[0].rows, [[stored.id, expected ? "expired" : stored.status]])
    && control.steps[0].outcome === "Claimed" && isDeepStrictEqual(control.steps[0].rows, [[positive.id, "expired"]]);
});

law("a unique grant set shared by several fields admits once", 81003, async (value) => {
  const grants = [grant(0, value.nullable), grant(1), grant(2)];
  // These are the SQL inputs after Rust's native-tested operation deduplication.
  const references = [0, 0, ...value.refs];
  const unique = [...new Set(references)];
  const claims = unique.map((index) => claim(grants[index]));
  const reply = await bridge.request(grants, [event(claims), event(claims)]);
  const expected = grants.map((stored, index) => [stored.id, unique.includes(index) ? "expired" : "allowed_once"]);
  return reply.steps[0].outcome === "Claimed" && reply.steps[1].outcome === "Unavailable"
    && reply.steps.every((step) => isDeepStrictEqual(step.rows, expected));
});

law("a failed multi-grant admission rolls back the entire set", 81004, async (value) => {
  const grants = [grant(0, value.nullable), grant(1)];
  const first = claim(grants[0]), second = claim(grants[1]);
  const failed = value.sqlError ? second : { ...second, host: "changed" };
  const reply = await bridge.request(grants, [event([first, failed], 1000, value.sqlError ? grants[1].id : null),
    event([first, second]), event([first, second])]);
  return reply.steps[0].outcome === (value.sqlError ? "SqlError" : "Unavailable")
    && isDeepStrictEqual(reply.steps[0].rows, rows(grants))
    && reply.steps[1].outcome === "Claimed" && reply.steps[2].outcome === "Unavailable"
    && reply.steps.slice(1).every((step) => isDeepStrictEqual(step.rows, grants.map((stored) => [stored.id, "expired"])));
});

test("bounded SQL bridge rejects wrong shapes and reports runtime provenance", () => {
  const valid = { id: 9, grants: [grant(0)], events: [] };
  const bad = ["{", JSON.stringify({ ...valid, extra: true }), JSON.stringify({ ...valid, grants: [] }),
    JSON.stringify({ ...valid, events: Array(33).fill(event([])) }), "x".repeat(65537)];
  const child = spawnSync("python3", args, { cwd: work, env: environment,
    input: [...bad, JSON.stringify(valid)].join("\n") + "\n", encoding: "utf8", timeout: 5000 });
  assert.equal(child.status, 0); assert.equal(child.stderr, "");
  const replies = child.stdout.trim().split("\n").map((line) => JSON.parse(line));
  assert.equal(replies.length, bad.length + 1);
  assert.ok(replies.slice(0, -1).every((reply) => reply.error === "Invalid trace" && Object.keys(reply).length === 2));
  const reply = decode(replies.at(-1)); assert.equal(reply.id, 9); assert.deepEqual(reply.steps, []);
  assert.match(reply.sql_sha256, /^[a-f0-9]{64}$/); assert.match(reply.python, /^3\./); assert.match(reply.sqlite, /^3\./);
  console.log("once-claim-runtime:" + JSON.stringify({ python: reply.python, sqlite: reply.sqlite, sql_sha256: reply.sql_sha256 }));
});

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

const script = fileURLToPath(new URL("../../scripts/release/publication_state_bridge.py", import.meta.url));
const work = mkdtempSync(join(tmpdir(), "arbos-publication-laws-"));
const environment = { PATH: process.env.PATH ?? "/usr/bin:/bin" };
const args = ["-I", "-B", script];
const target = Schema.Literals(["draft", "candidate", "edge", "stable"]);
const error = Schema.Literals(["invalid_publication_input", "release_observation_failed", "invalid_release_response",
  "release_command_timeout", "release_response_too_large", "release_command_failed", "invalid_publication_transition"]);
const terminal = Schema.Union([
  Schema.Struct({ _tag: Schema.Literal("Complete"), target }),
  Schema.Struct({ _tag: Schema.Literal("Refused") }),
  Schema.Struct({ _tag: Schema.Literal("Failed"), error }),
]);
const state = Schema.Union([
  terminal,
  Schema.Struct({ _tag: Schema.Literals(["Missing", "Draft", "Prerelease", "Stable"]) }),
  Schema.Struct({ _tag: Schema.Literals(["Creating", "Uploading", "Finalizing"]), target }),
]);
type State = typeof state.Type;
type Event = Readonly<Record<string, string>>;
const Reply = Schema.Struct({
  id: Schema.Int.check(Schema.isBetween({ minimum: 0, maximum: Number.MAX_SAFE_INTEGER })),
  initial: state, steps: Schema.Array(state),
});
type Reply = typeof Reply.Type;
const decode = Schema.decodeUnknownSync(Reply, { onExcessProperty: "error" });

/** One persistent pure-transition child; every request starts from an explicit actual state. */
class PublicationBridge {
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

  request(state: State, events: readonly Event[]): Promise<Reply> {
    const next = this.tail.then(() => {
      if (this.failed || this.closing) throw new Error("Pure publication oracle unavailable");
      const id = this.nextId++;
      const line = JSON.stringify({ id, state, events });
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
    this.failed ??= new Error("Pure publication oracle failed");
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



const bridge = new PublicationBridge();
after(async () => { await bridge.close(); rmSync(work, { recursive: true, force: true }); });
const channel = Schema.Literals(["stable", "edge"]);
const event = Schema.Union([
  Schema.Struct({ kind: Schema.Literal("request"), intent: Schema.Literals(["no-promote", "publish"]), channel }),
  Schema.Struct({ kind: Schema.Literals(["created", "uploaded", "finalized"]) }),
  Schema.Struct({ kind: Schema.Literal("failure"), error }),
]);
const input = Schema.Struct({ terminal, channel, events: Schema.Array(event).check(Schema.isMaxLength(24)) });
type Input = typeof input.Type;
const canonical: Input = { terminal: { _tag: "Complete", target: "candidate" }, channel: "stable", events: [] };
const arbitrary = Arbitrary.schema(input, { shrink: (value) => isDeepStrictEqual(value, canonical) ? [] : [canonical] });

function law(name: string, seed: number, predicate: (value: Input) => Promise<boolean>) {
  test(`${name}: 256 seeded traces`, async () => {
    const property = (value: Input) => Effect.tryPromise({ try: () => predicate(value),
      catch: () => new Error("Publication state observation failed") });
    const result = await Effect.runPromise(Arbitrary.checkEffect(arbitrary, property,
      { runs: 256, seed, size: 64, maxShrinks: 256, concurrency: 1 }));
    if (result._tag === "Falsified") {
      const replay = await Effect.runPromise(Arbitrary.checkEffect(arbitrary, property, { replay: result.replay }));
      assert.equal(replay._tag, "Falsified", "recorded counterexample must replay");
      if (replay._tag === "Falsified") assert.deepEqual(replay.shrunkInput, result.shrunkInput);
      console.log("publication-counterexample:" + Buffer.from(JSON.stringify({ law: name, seed, result, replay })).toString("base64"));
    }
    assert.equal(result._tag, "Passed", Arbitrary.formatCheckFailure(result));
    if (result._tag === "Passed") assert.equal(result.runs, 256);
  });
}

law("publication terminals absorb every later event", 84801, async (value) => {
  const reply = await bridge.request(value.terminal, [
    { kind: "request", intent: "publish", channel: value.channel },
    { kind: "created" }, { kind: "uploaded" }, { kind: "finalized" },
    { kind: "failure", error: "release_command_failed" }, ...value.events,
  ]);
  return reply.steps.every((step) => isDeepStrictEqual(step, reply.initial));
});

law("stable NoPromote refuses every mutation transition", 84802, async (value) => {
  const reply = await bridge.request({ _tag: "Stable" }, [
    { kind: "request", intent: "no-promote", channel: value.channel },
    { kind: "uploaded" }, { kind: "finalized" }, { kind: "created" }, ...value.events,
  ]);
  return reply.steps.every((step) => step._tag === "Refused");
});

law("draft completion truthfully retains or publishes its requested intent", 84803, async (value) => {
  for (const intent of ["publish", "no-promote"]) {
    const expected = intent === "no-promote" ? "draft" : value.channel === "stable" ? "stable" : "edge";
    const reply = await bridge.request({ _tag: "Draft" }, [
      { kind: "request", intent, channel: value.channel }, { kind: "uploaded" }, { kind: "finalized" }, ...value.events,
    ]);
    if (!isDeepStrictEqual(reply.steps[0], { _tag: "Uploading", target: expected })
      || !isDeepStrictEqual(reply.steps[1], { _tag: "Finalizing", target: expected })
      || !isDeepStrictEqual(reply.steps[2], { _tag: "Complete", target: expected })
      || !reply.steps.slice(2).every((step) => isDeepStrictEqual(step, reply.steps[2]))) return false;
  }
  return true;
});

test("pure publication bridge rejects invalid traces and recovers", () => {
  const valid = { id: 8, state: { _tag: "Stable" }, events: [] };
  const bad = ["{", JSON.stringify({ ...valid, extra: true }),
    JSON.stringify({ ...valid, state: { _tag: "Creating", target: "draft" } }),
    JSON.stringify({ ...valid, events: [{ kind: "request", intent: true, channel: "stable" }] }),
    JSON.stringify({ ...valid, events: Array(65).fill({ kind: "created" }) }), "x".repeat(65537)];
  const child = spawnSync("python3", args, { cwd: work, env: environment,
    input: [...bad, JSON.stringify(valid)].join("\n") + "\n", encoding: "utf8", timeout: 5000 });
  assert.equal(child.status, 0);
  assert.equal(child.stderr, "");
  const replies = child.stdout.trim().split("\n").map((line) => JSON.parse(line));
  assert.equal(replies.length, bad.length + 1);
  assert.ok(replies.slice(0, -1).every((reply) => reply.error === "Invalid trace" && Object.keys(reply).length === 2));
  assert.deepEqual(decode(replies.at(-1)), { id: 8, initial: { _tag: "Stable" }, steps: [] });
});

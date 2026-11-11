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

const root = fileURLToPath(new URL("../../", import.meta.url));
const leaf = join(root, ".improve/state-machine-checks/pending-owners");
const manifest = join(leaf, "native/Cargo.toml");
const work = mkdtempSync(join(tmpdir(), "arbos-pending-laws-"));
const environment: Record<string, string> = { PATH: process.env.PATH ?? "/usr/bin:/bin" };
// Build dependencies are cached by CI before its no-egress fence. No product configuration is read.
for (const key of ["CARGO_HOME", "RUSTUP_HOME"]) if (process.env[key]) environment[key] = process.env[key]!;
const hash = (file: string) => createHash("sha256").update(readFileSync(file)).digest("hex");
const sourcePaths = ["crates/arbos-agentd/src/approval.rs", "crates/arbos-agentd/src/store.rs", "crates/arbos-core/src/approval.rs", "crates/arbos-core/src/ids.rs", "crates/arbos-relay/src/state.rs", "crates/arbos-relay/src/restart_lifetime.rs"];
const hashes = () => Object.fromEntries(sourcePaths.map(p => [p, hash(join(root, p))]));
const before = hashes();
function run(command: string, args: string[]) {
    const result = spawnSync(command, args, { cwd: work, env: environment, encoding: "utf8", timeout: 120000, maxBuffer: 1024 * 1024 });
    assert.equal(result.status, 0, `Unable to prepare actual ownership bridge (${command})`);
}
run("python3", ["-I", "-B", join(leaf, "extract.py"), "--root", root, "--output", join(leaf, "native/src/generated.rs")]);
// One target belongs to this leaf/worktree. Clean the package so a copied newer artifact cannot stand in for changed source.
run("cargo", ["clean", "--offline", "--manifest-path", manifest, "-p", "pending-owner-bridge"]);
run("cargo", ["build", "--locked", "--offline", "--manifest-path", manifest, "--jobs", "2"]);
const binary = join(leaf, "native/target/debug/pending-owner-bridge");
const integer = Schema.Int.check(Schema.isBetween({ minimum: 0, maximum: Number.MAX_SAFE_INTEGER }));
const nullableNumber = Schema.NullOr(integer);
const deliveryStep = Schema.Struct({ mode: Schema.Literals(["Inline", "Polled", "Deferred"]), sender_closed: Schema.NullOr(Schema.Boolean), receiver: Schema.Literals(["Absent", "Open", "Closed", "Delivered"]), action: Schema.Literals(["Parked", "Unchanged", "Inline", "Polled", "Deferred", "ReceiverDropped", "Observed"]) });
const approvalRow = Schema.Struct({ id: Schema.Literal("fixture"), chat_id: Schema.Literal("synthetic-chat"), message_id: Schema.Literal("synthetic-message"), title: Schema.Literal("Synthetic card"), risk: Schema.Literal("other"), payload: Schema.Struct({}), state: Schema.Literals(["pending", "approved", "rejected", "cancelled", "expired"]), created_at: integer, expires_at: integer, decided_at: nullableNumber, decided_by: Schema.NullOr(Schema.String), reason: Schema.NullOr(Schema.String), grant_consumed_at: nullableNumber });
const approvalStep = Schema.Struct({ result: Schema.Literals(["True", "False", "Error", "Fixture", "Observed"]), row: approvalRow });
const actor = Schema.Struct({ id: Schema.Int.check(Schema.isBetween({ minimum: 0, maximum: 4 })), user: Schema.Literals(["a", "b"]), holding: Schema.Boolean });
const creationStep = Schema.Struct({ action: Schema.Literals(["Started", "Existing", "Dropped", "Absent", "DroppedAll", "Polled"]), actors: Schema.Array(actor).check(Schema.isMaxLength(5)), markers: Schema.Array(Schema.Literals(["a", "b"])).check(Schema.isMaxLength(2)) });
const bounded = <S extends Schema.Top>(s: S) => Schema.Array(s).check(Schema.isMaxLength(64));
const Delivery = Schema.Struct({ id: integer, observations: bounded(deliveryStep) });
const Approval = Schema.Struct({ id: integer, observations: bounded(approvalStep) });
const Creation = Schema.Struct({ id: integer, observations: Schema.Struct({ steps: bounded(creationStep), remaining_markers: integer }) });
const decodeDelivery = Schema.decodeUnknownSync(Delivery, { onExcessProperty: "error" });
const decodeApproval = Schema.decodeUnknownSync(Approval, { onExcessProperty: "error" });
const decodeCreation = Schema.decodeUnknownSync(Creation, { onExcessProperty: "error" });
type DeliveryStep = typeof deliveryStep.Type;
type ApprovalStep = typeof approvalStep.Type;
type CreationStep = typeof creationStep.Type;
/** Persistent owned local process; bounded, serialized request/reply transport. */
class OwnershipBridge {
    private readonly child = spawn(binary, [], { cwd: work, env: environment, stdio: "pipe" });
    private nextId = 0;
    private tail: Promise<unknown> = Promise.resolve();
    private buffer = "";
    private failed: Error | undefined;
    private closing = false;
    private readonly exited: Promise<void>;
    private pending: { id: number; resolve: (value: unknown) => void; reject: (error: Error) => void; timer: NodeJS.Timeout } | undefined;
    constructor() {
        this.exited = new Promise(resolve => {
            this.child.once("close", () => { if (!this.closing) this.fail(); resolve(); });
            this.child.once("error", () => { this.fail(); resolve(); });
        });
        this.child.stdin.on("error", () => this.fail());
        this.child.stderr.on("data", () => this.fail());
        this.child.stdout.setEncoding("utf8");
        this.child.stdout.on("data", (chunk: string) => {
            this.buffer += chunk;
            if (Buffer.byteLength(this.buffer) > 262144) return this.fail();
            for (;;) {
                const end = this.buffer.indexOf("\n");
                if (end < 0) return;
                const line = this.buffer.slice(0, end); this.buffer = this.buffer.slice(end + 1);
                try {
                    const value: unknown = JSON.parse(line);
                    if (!this.pending || typeof value !== "object" || value === null || !("id" in value) || value.id !== this.pending.id) throw new Error();
                    const pending = this.pending; this.pending = undefined; clearTimeout(pending.timer); pending.resolve(value);
                } catch { this.fail(); return; }
            }
        });
    }
    request(value: object): Promise<unknown> {
        const next = this.tail.then(() => {
            if (this.failed || this.closing) throw new Error("Ownership bridge unavailable");
            const id = this.nextId++; const line = JSON.stringify({ ...value, id });
            if (Buffer.byteLength(line) > 8192) throw new Error("Trace exceeds bound");
            return new Promise<unknown>((resolve, reject) => {
                this.pending = { id, resolve, reject, timer: setTimeout(() => this.fail(), 5000) };
                this.child.stdin.write(line + "\n");
            });
        });
        this.tail = next.catch(() => undefined); return next;
    }
    private fail() {
        this.failed ??= new Error("Ownership observation failed");
        if (this.pending) { clearTimeout(this.pending.timer); this.pending.reject(this.failed); this.pending = undefined; }
        this.child.kill();
    }
    async close() {
        this.closing = true; this.child.stdin.end();
        const timer = setTimeout(() => this.child.kill("SIGKILL"), 1000);
        try { await this.exited; } finally { clearTimeout(timer); }
    }
}
const bridge = new OwnershipBridge();
after(async () => { try { await bridge.close(); assert.deepEqual(hashes(), before); } finally { rmSync(work, { recursive: true, force: true }); } });
async function delivery(initial: string, events: number[]): Promise<readonly DeliveryStep[]> {
    const reply = decodeDelivery(await bridge.request({ op: "delivery", initial, events })); assert.equal(reply.observations.length, events.length); return reply.observations;
}
async function approval(events: number[], legacy_expired = false): Promise<readonly ApprovalStep[]> {
    const reply = decodeApproval(await bridge.request({ op: "approval", legacy_expired, events })); assert.equal(reply.observations.length, events.length); return reply.observations;
}
async function creation(events: number[]) {
    const reply = decodeCreation(await bridge.request({ op: "creation", events })); assert.equal(reply.observations.steps.length, events.length); return reply.observations;
}
const input = Schema.Struct({ choice: Schema.Int.check(Schema.isBetween({ minimum: 0, maximum: 3 })), fault: Schema.Int.check(Schema.isBetween({ minimum: 0, maximum: 1 })), tail: Schema.Array(Schema.Int.check(Schema.isBetween({ minimum: 0, maximum: 12 }))).check(Schema.isMaxLength(24)) });
type Input = typeof input.Type;
const canonical: Input = { choice: 0, fault: 0, tail: [] };
const arbitrary = Arbitrary.schema(input, { shrink: value => {
    const candidates = [{ ...value, tail: [] }, { choice: 0, fault: value.fault, tail: [] }, canonical];
    return candidates.filter((candidate, index) => !isDeepStrictEqual(candidate, value) && candidates.findIndex(other => isDeepStrictEqual(candidate, other)) === index);
} });
function law(name: string, seed: number, predicate: (input: Input) => Promise<boolean>) {
    if (process.env.ARBOS_PENDING_LAW && process.env.ARBOS_PENDING_LAW !== String(seed)) return;
    test(`${name}: 256 seeded traces`, async () => {
        if (process.env.ARBOS_PENDING_REPLAY) { const value = Schema.decodeUnknownSync(input, { onExcessProperty: "error" })(JSON.parse(process.env.ARBOS_PENDING_REPLAY)); assert.equal(await predicate(value), true, "Exact counterexample replay"); return; }
        const property = (value: Input) => Effect.tryPromise({ try: () => predicate(value), catch: () => new Error("Ownership observation failed") });
        const result = await Effect.runPromise(Arbitrary.checkEffect(arbitrary, property, { runs: 256, seed, size: 64, maxShrinks: 256, concurrency: 1 }));
        if (result._tag === "Falsified") {
            const replay = await Effect.runPromise(Arbitrary.checkEffect(arbitrary, property, { replay: result.replay })); assert.equal(replay._tag, "Falsified");
            if (replay._tag === "Falsified") assert.deepEqual(replay.shrunkInput, result.shrunkInput);
            console.log("pending-owner-counterexample:" + Buffer.from(JSON.stringify({ name, seed, result, replay })).toString("base64"));
        }
        assert.equal(result._tag, "Passed", Arbitrary.formatCheckFailure(result)); if (result._tag === "Passed") assert.equal(result.runs, 256);
    });
}
law("park is absorbing and closed inline receivers defer before admission", 94001, async value => {
    const live = await delivery("inline", [3, 1]); const polled = await delivery("polled", [3, 1]);
    const closed = await delivery("inline", [2, 1]);
    const tail = value.tail.map(n => n % 4);
    const parked = await delivery(["inline", "polled", "deferred"][value.choice % 3], [0, ...tail, 1]);
    return live[1].action === "Inline" && live[1].receiver === "Open" && polled[1].action === "Polled" && closed[0].sender_closed === true && closed[1].action === "Deferred" && parked.every(s => s.mode === "Deferred") && parked.at(-1)?.action === "Deferred";
});
law("durable first decision owns at most one grant and terminal metadata", 94002, async value => {
    const first = value.choice;
    const events = [9, first, ...value.tail.map(n => n % 5), 4, 4];
    const steps = await approval(events); const committed = steps[1].row;
    let consumed = committed.grant_consumed_at;
    if (steps[0].row.state !== "pending" || steps[1].result !== "True" || committed.state !== ["approved", "approved", "rejected", "cancelled"][first]) return false;
    if ((first === 0) !== (consumed !== null)) return false;
    let successfulClaims = first === 0 ? 1 : 0;
    for (let i = 2; i < steps.length; i++) {
        const step = steps[i];
        if (step.row.state !== committed.state || step.row.decided_at !== committed.decided_at || step.row.decided_by !== committed.decided_by || step.row.reason !== committed.reason) return false;
        if (events[i] === 4 && step.result === "True") successfulClaims++;
        if (consumed !== null && step.row.grant_consumed_at !== consumed) return false;
        consumed = step.row.grant_consumed_at;
    }
    return successfulClaims === (first <= 1 ? 1 : 0) && steps.at(-1)?.result === "False";
});
law("claim SQL failures roll back admission and legacy terminal cards stay closed", 94003, async value => {
    const events = [9, value.fault === 0 ? 5 : 6, 0, 9, 7, 8, 0, ...value.tail.map(n => n % 5), 4];
    const steps = await approval(events);
    const pending = steps[0].row;
    if (steps[2].result !== "Error" || !isDeepStrictEqual(steps[2].row, pending) || !isDeepStrictEqual(steps[3].row, pending) || steps[5].result !== "Error" || !isDeepStrictEqual(steps[5].row, pending)) return false;
    if (steps[6].result !== "True" || steps[6].row.state !== "approved" || steps[6].row.grant_consumed_at === null) return false;
    if (!steps.slice(6).every(s => isDeepStrictEqual(s.row, steps[6].row))) return false;
    const legacy = await approval([9, ...value.tail.map(n => n % 5), 0, 4], true);
    return legacy.every(s => isDeepStrictEqual(s.row, legacy[0].row)) && legacy[0].row.state === "expired" && legacy.at(-1)?.result === "False";
});
function validCreation(steps: readonly CreationStep[]) {
    return steps.every(s => {
        const holding = s.actors.filter(a => a.holding).map(a => a.user);
        return new Set(s.actors.map(a => a.id)).size === s.actors.length && new Set(holding).size === holding.length && isDeepStrictEqual([...holding].sort(), [...s.markers].sort());
    });
}
law("creation permits exclude same-user owners while independent users progress", 94004, async value => {
    const result = await creation([0, 1, 2, ...value.tail, 11]);
    const first = result.steps[2];
    return first.actors.filter(a => a.user === "a" && a.holding).length === 1 && first.actors.find(a => a.id === 1)?.holding === false && first.actors.find(a => a.id === 2)?.holding === true && validCreation(result.steps) && result.remaining_markers === 0 && result.steps.at(-1)?.markers.length === 0;
});
law("cancelling a waiter preserves its owner and releasing the owner admits its successor", 94005, async value => {
    const result = await creation([0, 1, 2, 4, 7, 3, ...value.tail, 11]);
    const cancelled = result.steps[3], successor = result.steps[5];
    return cancelled.actors.find(a => a.id === 0)?.holding === true && isDeepStrictEqual(cancelled.markers, ["a", "b"]) && successor.actors.find(a => a.id === 3)?.holding === true && successor.actors.find(a => a.id === 2)?.holding === true && validCreation(result.steps) && result.remaining_markers === 0;
});
test("bounded ownership bridge rejects invalid requests, drains oversized lines and handles EOF", () => {
    const valid = { id: 19, op: "creation", events: [0, 1, 2, 11] };
    const bad = ["not-json", "[]", JSON.stringify({ ...valid, extra: "no" }), JSON.stringify({ ...valid, id: -1 }), JSON.stringify({ ...valid, op: "unknown" }), JSON.stringify({ ...valid, events: [13] }), JSON.stringify({ ...valid, events: Array(65).fill(0) }), "x".repeat(8193), JSON.stringify({ id: 1, op: "delivery", initial: "unknown", events: [] })];
    const child = spawnSync(binary, [], { cwd: work, env: environment, input: [...bad, JSON.stringify(valid)].join("\n"), encoding: "utf8", timeout: 5000, maxBuffer: 262144 });
    assert.equal(child.status, 0); assert.equal(child.stderr, "");
    const replies = child.stdout.trim().split("\n").map(line => JSON.parse(line)); assert.equal(replies.length, bad.length + 1);
    assert.ok(replies.slice(0, -1).every(value => isDeepStrictEqual(value, { id: null, error: "Invalid trace" })));
    const result = decodeCreation(replies.at(-1)); assert.equal(result.id, 19); assert.equal(result.observations.steps.length, 4); assert.equal(result.observations.remaining_markers, 0);
});

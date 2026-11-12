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
const leaf = join(root, ".improve/state-machine-checks/voice-settlement");
const manifest = join(leaf, "native/Cargo.toml");
const work = mkdtempSync(join(tmpdir(), "arbos-settlement-laws-"));
const environment: Record<string, string> = { PATH: process.env.PATH ?? "/usr/bin:/bin" };
// Build dependencies are cached by CI before its no-egress fence. No product configuration is read.
for (const key of ["CARGO_HOME", "RUSTUP_HOME"]) if (process.env[key]) environment[key] = process.env[key]!;
const hash = (file: string) => createHash("sha256").update(readFileSync(file)).digest("hex");
const sourcePaths = ["crates/arbos-relay/src/db.rs", "crates/arbos-relay/src/db_accounts.rs", "crates/arbos-relay/src/db_voice.rs", "crates/arbos-relay/src/credits.rs", "crates/arbos-relay/src/proxy.rs", "crates/arbos-core/src/ids.rs"];
const hashes = () => Object.fromEntries(sourcePaths.map(p => [p, hash(join(root, p))]));
const before = hashes();
function run(command: string, args: string[]) {
    const result = spawnSync(command, args, { cwd: work, env: environment, encoding: "utf8", timeout: 120000, maxBuffer: 1024 * 1024 });
    assert.equal(result.status, 0, `Unable to prepare actual ownership bridge (${command})`);
}
run("python3", ["-I", "-B", join(leaf, "extract.py"), "--root", root, "--output", join(leaf, "native/src/generated.rs")]);
// One target belongs to this leaf/worktree. Clean the package so a copied newer artifact cannot stand in for changed source.
run("cargo", ["clean", "--offline", "--manifest-path", manifest, "-p", "voice-settlement-bridge"]);
run("cargo", ["build", "--locked", "--offline", "--manifest-path", manifest, "--jobs", "2"]);
const binary = join(leaf, "native/target/debug/voice-settlement-bridge");
const integer = Schema.Int.check(Schema.isBetween({ minimum: 0, maximum: Number.MAX_SAFE_INTEGER }));
const amount = Schema.Number.check(Schema.isBetween({ minimum: -1_000_000, maximum: 1_000_000 }));
const account = Schema.Struct({ cap: amount, unlimited: Schema.Boolean, allow_negative: Schema.Boolean });
const ledgerRow = Schema.Struct({ kind: Schema.Literals(["grant", "voice"]), usd: amount, reference: Schema.NullOr(Schema.Literals(["call:synthetic-call", "call:synthetic-call#vision"])) });
const usage = Schema.Struct({ total: amount, vision: amount, settled: amount, vision_settled: amount });
const snapshot = Schema.Struct({ account: Schema.NullOr(account), ledger: Schema.Array(ledgerRow).check(Schema.isMaxLength(256)), usage: Schema.NullOr(usage) });
const debit = Schema.Union([Schema.Struct({ tag: Schema.Literal("Paid") }), Schema.Struct({ tag: Schema.Literal("Clamped"), balance: amount }), Schema.Struct({ tag: Schema.Literal("Refused"), reason: Schema.Literals(["Insufficient", "DailyCap", "ReadOnly"]) })]);
const charge = Schema.Struct({ reported_part: amount, legacy: Schema.Literals(["Existing", "Granted", "Unfunded"]), debit });
const result = Schema.Union([Schema.Struct({ tag: Schema.Literal("Fixture") }), Schema.Struct({ tag: Schema.Literal("Error") }), Schema.Struct({ tag: Schema.Literal("Committed"), reported_total: amount, reported_delta: amount, charges: Schema.Array(charge).check(Schema.isMaxLength(2)) })]);
const step = Schema.Struct({ result, snapshot });
const Reply = Schema.Struct({ id: integer, sqlite_version: Schema.String.check(Schema.isMaxLength(32)), initial: snapshot, steps: Schema.Array(step).check(Schema.isMaxLength(64)) });
const decode = Schema.decodeUnknownSync(Reply, { onExcessProperty: "error" });
type Reply = typeof Reply.Type;
type Snapshot = typeof snapshot.Type;
type Config = { existing: boolean; metered: boolean; grant_micros: number; cap_micros: number | null; allow_negative: boolean };
type Event = { op: "report"; total_micros: number; vision_micros: number } | { op: "fault"; kind: number } | { op: "clear" };
/** Persistent owned local process; bounded, serialized request/reply transport. */
class SettlementBridge {
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
const bridge = new SettlementBridge();
after(async () => { try { await bridge.close(); assert.deepEqual(hashes(), before); } finally { rmSync(work, { recursive: true, force: true }); } });
const config: Config = { existing: false, metered: true, grant_micros: 5_000_000, cap_micros: null, allow_negative: false };
const report = (total_micros: number, vision_micros = 0): Event => ({ op: "report", total_micros, vision_micros });
async function observe(config: Config, events: Event[]): Promise<Reply> {
    const result = decode(await bridge.request({ config, events })); assert.equal(result.steps.length, events.length); return result;
}
const near = (a: number, b: number) => Math.abs(a - b) < 1e-8;
const spent = (s: Snapshot) => -s.ledger.filter(row => row.usd < 0).reduce((sum, row) => sum + row.usd, 0);
const balance = (s: Snapshot) => s.ledger.reduce((sum, row) => sum + row.usd, 0);
const input = Schema.Struct({ choice: Schema.Int.check(Schema.isBetween({ minimum: 0, maximum: 6 })), units: Schema.Int.check(Schema.isBetween({ minimum: 0, maximum: 200_000_000 })), tail: Schema.Array(Schema.Int.check(Schema.isBetween({ minimum: 0, maximum: 100_000_000 }))).check(Schema.isMaxLength(12)) });
type Input = typeof input.Type;
const canonical: Input = { choice: 0, units: 0, tail: [] };
const arbitrary = Arbitrary.schema(input, { shrink: value => {
    const candidates = [{ ...value, tail: [] }, { choice: value.choice, units: 0, tail: [] }, canonical];
    return candidates.filter((candidate, index) => !isDeepStrictEqual(candidate, value) && candidates.findIndex(other => isDeepStrictEqual(candidate, other)) === index);
} });
function law(name: string, seed: number, predicate: (input: Input) => Promise<boolean>) {
    if (process.env.ARBOS_SETTLEMENT_LAW && process.env.ARBOS_SETTLEMENT_LAW !== String(seed)) return;
    test(`${name}: 256 seeded traces`, async () => {
        if (process.env.ARBOS_SETTLEMENT_REPLAY) { const value = Schema.decodeUnknownSync(input, { onExcessProperty: "error" })(JSON.parse(process.env.ARBOS_SETTLEMENT_REPLAY)); assert.equal(await predicate(value), true, "Exact counterexample replay"); return; }
        const property = (value: Input) => Effect.tryPromise({ try: () => predicate(value), catch: () => new Error("Settlement observation failed") });
        const result = await Effect.runPromise(Arbitrary.checkEffect(arbitrary, property, { runs: 256, seed, size: 64, maxShrinks: 256, concurrency: 1 }));
        if (result._tag === "Falsified") {
            const replay = await Effect.runPromise(Arbitrary.checkEffect(arbitrary, property, { replay: result.replay })); assert.equal(replay._tag, "Falsified");
            if (replay._tag === "Falsified") assert.deepEqual(replay.shrunkInput, result.shrunkInput);
            console.log("voice-settlement-counterexample:" + Buffer.from(JSON.stringify({ name, seed, result, replay })).toString("base64"));
        }
        assert.equal(result._tag, "Passed", Arbitrary.formatCheckFailure(result)); if (result._tag === "Passed") assert.equal(result.runs, 256);
    });
}
law("settlement SQL faults roll back receipt debit and initialization together", 95001, async value => {
    const amount = 1_000_000 + value.units % 1_000_000;
    const events: Event[] = [{ op: "fault", kind: value.choice }, report(amount), { op: "clear" }, report(amount), report(amount)];
    const reply = await observe(config, events); const failed = reply.steps[1], retry = reply.steps[3], duplicate = reply.steps[4];
    return failed.result.tag === "Error" && isDeepStrictEqual(failed.snapshot, reply.initial) && retry.result.tag === "Committed" && near(retry.result.reported_delta, amount / 1e6) && near(spent(retry.snapshot), amount / 1e6) && retry.snapshot.account?.unlimited === true && duplicate.result.tag === "Committed" && duplicate.result.reported_delta === 0 && isDeepStrictEqual(duplicate.snapshot, retry.snapshot);
});
law("reordered and repeated cumulative reports never duplicate clamped ledger debits", 95002, async value => {
    const grant = 1_000_000 + value.units % 10_000_000;
    const first = report(grant * 2, Math.floor(grant / 2));
    const reports = [first, ...value.tail.map((total, index) => report(total, index % 2 ? Math.floor(total / 3) : 0))];
    const events = reports.flatMap(event => [event, event]);
    const reply = await observe({ ...config, existing: true, grant_micros: grant }, events);
    const initialCharge = reply.steps[0].result;
    if (initialCharge.tag !== "Committed" || initialCharge.charges[0]?.debit.tag !== "Clamped" || !near(initialCharge.reported_total, grant * 2 / 1e6) || !near(spent(reply.steps[0].snapshot), grant / 1e6) || reply.steps[0].snapshot.ledger.filter(row => row.usd < 0)[0]?.reference !== "call:synthetic-call" || reply.steps[0].snapshot.ledger.filter(row => row.usd < 0).length !== 1) return false;
    for (let i = 0; i < reply.steps.length; i += 2) {
        const a = reply.steps[i], b = reply.steps[i + 1];
        if (a.result.tag !== "Committed" || b.result.tag !== "Committed" || b.result.reported_delta !== 0 || !isDeepStrictEqual(a.snapshot, b.snapshot) || !near(spent(b.snapshot), grant / 1e6) || balance(b.snapshot) < -1e-9 || b.snapshot.usage!.total - b.snapshot.usage!.settled > 1.0001e-6 || b.snapshot.usage!.settled - b.snapshot.usage!.total > 1e-9) return false;
    } return true;
});
law("strict per-part micro thresholds preserve legacy and unmetered initialization policy", 95003, async value => {
    const metered = value.choice % 2 === 0;
    for (const [total, vision] of [[0, 0], [1, 0], [2, 1], [2, 0], [3, 1], [4, 2]]) {
        const reply = await observe({ ...config, metered }, [report(total, vision), report(total, vision)]);
        const first = reply.steps[0], last = reply.steps[1];
        const initialized = metered && (total - vision > 1 || vision > 1);
        const expectedSpend = initialized ? (total - vision > 1 ? total - vision : 0) / 1e6 + (vision > 1 ? vision / 1e6 : 0) : 0;
        if (first.result.tag !== "Committed" || last.result.tag !== "Committed" || last.result.reported_delta !== 0 || !isDeepStrictEqual(first.snapshot, last.snapshot) || (first.snapshot.account !== null) !== initialized || !near(spent(first.snapshot), expectedSpend) || !near(first.snapshot.usage!.settled, total > 1 ? total / 1e6 : 0)) return false;
    } return true;
});
law("policy refusals settle without retry debit and retain soft-cap override and zero-grant behavior", 95004, async value => {
    const extra = 1_000_000 + value.units % 1_000_000;
    const soft = await observe({ ...config, existing: true, grant_micros: 10_000_000, cap_micros: 1_000_000 }, [report(2_000_000), report(2_000_000 + extra), report(2_000_000 + extra)]);
    const refusal = soft.steps[1].result;
    if (!near(spent(soft.steps[0].snapshot), 2) || refusal.tag !== "Committed" || refusal.charges[0]?.debit.tag !== "Refused" || refusal.charges[0].debit.reason !== "DailyCap" || !near(soft.steps[1].snapshot.usage!.settled, 2 + extra / 1e6) || !isDeepStrictEqual(soft.steps[1].snapshot, soft.steps[2].snapshot) || !near(spent(soft.steps[1].snapshot), 2)) return false;
    const override = await observe({ ...config, existing: true, grant_micros: 0, allow_negative: true }, [report(extra)]);
    if (!near(balance(override.steps[0].snapshot), -extra / 1e6)) return false;
    const zero = await observe({ ...config, grant_micros: 0 }, [report(extra), report(extra)]);
    const zeroReceipt = zero.steps[0].result;
    return zeroReceipt.tag === "Committed" && zeroReceipt.charges[0]?.legacy === "Unfunded" && zeroReceipt.charges[0].debit.tag === "Refused" && zeroReceipt.charges[0].debit.reason === "Insufficient" && zero.steps[0].snapshot.account !== null && zero.steps[0].snapshot.ledger.length === 0 && isDeepStrictEqual(zero.steps[0].snapshot, zero.steps[1].snapshot);
});
test("bounded settlement protocol rejects unknown invalid and oversized traces and recovers at EOF", () => {
    const valid = { id: 23, config, events: [report(1_000_000), report(1_000_000)] };
    const bad = ["not-json", "[]", JSON.stringify({ ...valid, extra: true }), JSON.stringify({ ...valid, id: -1 }), JSON.stringify({ ...valid, config: { ...config, unknown: true } }), JSON.stringify({ ...valid, events: [{ op: "fault", kind: 7 }] }), JSON.stringify({ ...valid, events: [report(1, 2)] }), JSON.stringify({ ...valid, events: Array(65).fill({ op: "clear" }) }), "x".repeat(8193)];
    const child = spawnSync(binary, [], { cwd: work, env: environment, input: [...bad, JSON.stringify(valid)].join("\n"), encoding: "utf8", timeout: 5000, maxBuffer: 262144 });
    assert.equal(child.status, 0); assert.equal(child.stderr, "");
    const replies = child.stdout.trim().split("\n").map(line => JSON.parse(line)); assert.equal(replies.length, bad.length + 1);
    assert.ok(replies.slice(0, -1).every(value => isDeepStrictEqual(value, { id: null, error: "Invalid trace" })));
    const result = decode(replies.at(-1)); assert.equal(result.id, 23); assert.equal(result.steps.length, 2); assert.equal(result.steps[1].result.tag, "Committed");
    if (result.steps[1].result.tag === "Committed") assert.equal(result.steps[1].result.reported_delta, 0);
});

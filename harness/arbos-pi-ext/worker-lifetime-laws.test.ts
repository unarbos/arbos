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
const source = fileURLToPath(new URL("../../.improve/state-machine-checks/worker_lifetime_bridge.rs", import.meta.url));
const work = mkdtempSync(join(tmpdir(), "arbos-worker-laws-"));
const environment = { PATH: process.env.PATH ?? "/usr/bin:/bin" };
const binary = join(work, process.platform === "win32" ? "worker-lifetime-bridge.exe" : "worker-lifetime-bridge");
const product = fileURLToPath(new URL("../../crates/arbos-agentd/src/worker_lifetime.rs", import.meta.url));
const hash = (file: string) => createHash("sha256").update(readFileSync(file)).digest("hex");
const sources = { product: hash(product), bridge: hash(source) };
const build = spawnSync("rustc", ["--edition=2021", "--crate-name", "worker_lifetime_bridge", "-O", source, "-o", binary], { cwd: work, env: environment, encoding: "utf8", timeout: 30000 });
assert.equal(build.status, 0, "Unable to compile the actual pure worker registry bridge");
const Entry = Schema.Struct({ tag: Schema.Literals(["Missing", "Starting", "Ready", "Stopping"]), owner: Schema.NullOr(Schema.Int.check(Schema.isBetween({ minimum: 1, maximum: 64 }))), resource: Schema.NullOr(Schema.Int.check(Schema.isBetween({ minimum: 0, maximum: 100000 }))) });
const snapshot = Schema.Struct({ tag: Schema.Literals(["Accepting", "Closed"]), a: Entry, b: Entry, action: Schema.Literals(["Begun", "Stopping", "Retired", "Accepted", "Missing", "Wait", "Closed", "Rejected"]), returned: Schema.Array(Schema.Int).check(Schema.isMaxLength(64)) });
const Reply = Schema.Struct({ id: Schema.Int.check(Schema.isBetween({ minimum: 0, maximum: 4294967295 })), steps: Schema.Array(snapshot).check(Schema.isMaxLength(64)) });
type Reply = typeof Reply.Type;
const decode = Schema.decodeUnknownSync(Reply, { onExcessProperty: "error" });
/** Every request starts with a fresh actual worker registry; only local integer handles are used. */
class WorkerRegistryBridge {
    private readonly child = spawn(binary, [], { cwd: work, env: environment, stdio: "pipe" });
    private readonly exited: Promise<void>;
    private tail: Promise<unknown> = Promise.resolve();
    private nextId = 0;
    private failed: Error | undefined;
    private buffer = "";
    private closing = false;
    private pending: {
        id: number;
        count: number;
        resolve: (reply: Reply) => void;
        reject: (error: Error) => void;
        timer: NodeJS.Timeout;
    } | undefined;
    constructor() {
        this.exited = new Promise((resolve) => {
            this.child.once("close", () => { if (!this.closing)
                this.fail(); resolve(); });
            this.child.once("error", () => { this.fail(); resolve(); });
        });
        this.child.stdin.on("error", () => this.fail());
        this.child.stderr.on("data", () => { });
        this.child.stdout.setEncoding("utf8");
        this.child.stdout.on("data", (chunk: string) => {
            if (this.failed || this.closing)
                return;
            this.buffer += chunk;
            if (Buffer.byteLength(this.buffer) > 262144) {
                this.fail();
                return;
            }
            for (;;) {
                const end = this.buffer.indexOf("\n");
                if (end < 0)
                    return;
                const line = this.buffer.slice(0, end);
                this.buffer = this.buffer.slice(end + 1);
                try {
                    const pending = this.pending;
                    const reply = decode(JSON.parse(line));
                    if (!pending || reply.id !== pending.id || reply.steps.length !== pending.count)
                        throw new Error("Invalid response shape");
                    clearTimeout(pending.timer);
                    this.pending = undefined;
                    pending.resolve(reply);
                }
                catch {
                    this.fail();
                    return;
                }
            }
        });
    }
    request(events: readonly string[]): Promise<Reply> {
        const next = this.tail.then(() => {
            if (this.failed || this.closing)
                throw new Error("WorkerRegistry observation bridge unavailable");
            const id = this.nextId++;
            const line = `${id}|${events.join(",")}`;
            if (events.length > 64 || Buffer.byteLength(line) > 8192)
                throw new Error("Trace exceeds protocol bound");
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
        this.failed ??= new Error("WorkerRegistry observation bridge failed");
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
        try {
            await this.exited;
        }
        finally {
            clearTimeout(timer);
        }
    }
}
const bridge = new WorkerRegistryBridge();
after(async () => {
    try {
        await bridge.close();
        assert.deepEqual({ product: hash(product), bridge: hash(source) }, sources);
    }
    finally {
        rmSync(work, { recursive: true, force: true });
    }
});
const input = Schema.Struct({ tail: Schema.Array(Schema.Int.check(Schema.isBetween({ minimum: 0, maximum: 8 }))).check(Schema.isMaxLength(24)) });
type Input = typeof input.Type;
const canonical: Input = { tail: [] };
const arbitrary = Arbitrary.schema(input, { shrink: (value) => isDeepStrictEqual(value, canonical) ? [] : [canonical] });
function law(name: string, seed: number, predicate: (value: Input) => Promise<boolean>) {
    if (process.env.ARBOS_WORKER_LAW && process.env.ARBOS_WORKER_LAW !== String(seed))
        return;
    test(`${name}: 256 seeded traces`, async () => {
        const property = (value: Input) => Effect.tryPromise({ try: () => predicate(value), catch: () => new Error("WorkerRegistry observation failed") });
        if (process.env.ARBOS_WORKER_REPLAY) {
            const value = Schema.decodeUnknownSync(input, { onExcessProperty: "error" })(JSON.parse(process.env.ARBOS_WORKER_REPLAY));
            assert.equal(await predicate(value), true, "Exact counterexample replay");
            return;
        }
        const result = await Effect.runPromise(Arbitrary.checkEffect(arbitrary, property, { runs: 256, seed, size: 64, maxShrinks: 256, concurrency: 1 }));
        if (result._tag === "Falsified") {
            const replay = await Effect.runPromise(Arbitrary.checkEffect(arbitrary, property, { replay: result.replay }));
            assert.equal(replay._tag, "Falsified");
            if (replay._tag === "Falsified")
                assert.deepEqual(replay.shrunkInput, result.shrunkInput);
            console.log("worker-lifetime-counterexample:" + Buffer.from(JSON.stringify({ law: name, seed, result, replay })).toString("base64"));
        }
        assert.equal(result._tag, "Passed", Arbitrary.formatCheckFailure(result));
        if (result._tag === "Passed")
            assert.equal(result.runs, 256);
    });
}
law("terminal shutdown refuses all late admission and completion", 92001, async (value) => {
    const prefix = ["b:a", "c:a:1:11", "b:b", "c:b:2:22", "p:b:2", "s"];
    const tail = value.tail.map((n, i) => ["b:a", "p:a:1", "x:a:1", `c:a:1:${100 + i}`, "d:a:1", "t:a", "s", "b:b", "p:b:2"][n]);
    const r = await bridge.request([...prefix, ...tail, "c:a:1:99", "b:a"]);
    return r.steps[1].a.resource === 11 && r.steps[4].b.tag === "Ready" && isDeepStrictEqual([...r.steps[5].returned].sort((a, b) => a - b), [11, 22]) && r.steps.slice(5).every(s => s.tag === "Closed" && s.a.tag === "Missing" && s.b.tag === "Missing" && s.action !== "Begun" && s.action !== "Accepted") && r.steps.at(-2)?.action === "Rejected" && isDeepStrictEqual(r.steps.at(-2)?.returned, [99]);
});
law("stale callbacks cannot publish abort or finish a replacement", 92002, async (value) => {
    const prefix = ["b:a", "c:a:1:11", "p:a:1", "b:a", "c:a:2:22"];
    const stale = ["p:a:1", "x:a:1", "d:a:1", ...value.tail.map(n => ["p:a:1", "x:a:1", "d:a:1"][n % 3])];
    const r = await bridge.request([...prefix, ...stale, "p:a:2", "x:a:1", "t:a", "d:a:1", "d:a:2"]);
    return r.steps[2].a.tag === "Ready" && isDeepStrictEqual(r.steps[3].returned, [11]) && r.steps.slice(5, 5 + stale.length).every(s => s.a.tag === "Starting" && s.a.resource === 22 && s.a.owner === 2 && s.action === "Rejected" && s.returned.length === 0) && r.steps.at(-5)?.a.tag === "Ready" && r.steps.at(-4)?.a.resource === 22 && r.steps.at(-3)?.a.tag === "Stopping" && r.steps.at(-2)?.a.tag === "Stopping" && r.steps.at(-2)?.action === "Rejected" && r.steps.at(-1)?.a.tag === "Missing" && r.steps.at(-1)?.action === "Accepted";
});
law("stopping waits for its exact receipt while independent sessions progress", 92003, async (value) => {
    const prefix = ["b:a", "c:a:1:11", "p:a:1", "b:b", "c:b:2:22", "p:b:2", "t:a"];
    const waiting = ["b:a", "d:a:2", ...value.tail.map(n => ["b:a", "d:a:2", "p:a:2", "x:a:2"][n % 4])];
    const r = await bridge.request([...prefix, ...waiting, "b:b", "c:b:3:33", "p:b:3", "d:a:2", "d:a:1", "b:a", "c:a:4:44", "p:a:4"]);
    return r.steps[6].a.tag === "Stopping" && r.steps[6].a.resource === 11 && r.steps.slice(7, 7 + waiting.length).every(s => s.a.tag === "Stopping" && s.a.resource === 11 && s.b.tag === "Ready" && s.b.resource === 22 && ["Wait", "Rejected"].includes(s.action)) && r.steps.at(-6)?.b.tag === "Ready" && r.steps.at(-6)?.b.resource === 33 && r.steps.at(-6)?.a.tag === "Stopping" && r.steps.at(-5)?.action === "Rejected" && r.steps.at(-4)?.a.tag === "Missing" && r.steps.at(-1)?.a.tag === "Ready" && r.steps.at(-1)?.a.resource === 44;
});
law("shutdown returns published and captured pending cleanup resources", 92004, async (value) => {
    const prefix = ["b:a", "c:a:1:11", "p:a:1", "b:b", "c:b:2:22", "t:a"];
    const tail = value.tail.map(n => ["p:a:1", "p:b:2", "t:a", "t:b", "x:a:2", "x:b:1", "d:a:2", "d:b:1", "t:a"][n]);
    const r = await bridge.request([...prefix, ...tail, "s", "s"]);
    const before = r.steps.at(-3)!;
    return r.steps[2].a.tag === "Ready" && r.steps[4].b.tag === "Starting" && r.steps[5].a.tag === "Stopping" && isDeepStrictEqual(r.steps[5].returned, [11]) && before.a.resource === 11 && before.b.resource === 22 && isDeepStrictEqual([...r.steps.at(-2)!.returned].sort((a, b) => a - b), [11, 22]) && r.steps.at(-2)?.tag === "Closed" && r.steps.at(-1)?.returned.length === 0;
});
test("bounded native worker protocol rejects malformed traces and recovers", () => {
    const bad = ["broken", "1|unknown", "1|c:a:1:bad", "1|p:a:1", "1|b:unknown", "1|" + Array(65).fill("b:a").join(","), "x".repeat(8193)];
    const c = spawnSync(binary, [], { cwd: work, env: environment, input: [...bad, "9|b:a,c:a:1:11,p:a:1,s"].join("\n") + "\n", encoding: "utf8", timeout: 5000 });
    assert.equal(c.status, 0);
    assert.equal(c.stderr, "");
    const replies = c.stdout.trim().split("\n").map(line => JSON.parse(line));
    assert.equal(replies.length, bad.length + 1);
    assert.ok(replies.slice(0, -1).every(r => r.id === null && r.error === "Invalid trace" && Object.keys(r).length === 2));
    const result = decode(replies.at(-1));
    assert.equal(result.id, 9);
    assert.equal(result.steps.length, 4);
    assert.equal(result.steps.at(-1)?.tag, "Closed");
    assert.deepEqual(result.steps.at(-1)?.returned, [11]);
});

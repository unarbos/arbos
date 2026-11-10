import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";
import { Bridge } from "../bridge.ts";
import { requiredLaws } from "../catalog.ts";

const [baseline, mutant, name, output] = process.argv.slice(2);
const candidates = requiredLaws;
const law = name === "prose"
  ? candidates.find((candidate) => candidate.name === "nonsecret-text-preservation")
  : candidates.find((candidate) => candidate.name === name);
assert.ok(law, `Missing law ${name}`);

async function check(binary: string, replay?: string) {
  const bridge = new Bridge(binary);
  try {
    return await law!.check(bridge, replay);
  } finally {
    await bridge.close();
  }
}

const control = await check(baseline);
assert.equal(control._tag, "Passed", "Unmodified control must pass before attributing failure to a mutation");
if (control._tag === "Passed") assert.equal(control.runs, 256);
const failed = await check(mutant);
assert.equal(failed._tag, "Falsified", "Actual-function mutation survived its target property");
if (failed._tag !== "Falsified") throw new Error("Expected shrunk failure");
const replay = await check(mutant, failed.replay);
assert.equal(replay._tag, "Falsified", "Recorded failure must replay");
if (replay._tag === "Falsified") assert.deepEqual(replay.shrunkInput, failed.shrunkInput);
await writeFile(output, JSON.stringify({ law: law.name, seed: law.seed, control, mutation: failed, replay }, null, 2) + "\n");
console.log(JSON.stringify({ law: law.name, control: control._tag, mutant: failed._tag, shrinks: failed.shrinks, replay: replay._tag }));

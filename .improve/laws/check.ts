import { writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import * as Arbitrary from "effect/Arbitrary";
import { Bridge } from "./bridge.ts";
import { requiredLaws } from "./catalog.ts";

const args = process.argv.slice(2);
const isBoundary = args[0] === "--boundary";
if (args.length > (isBoundary ? 2 : 0)) throw new Error("Usage: node check.ts [--boundary [replay-token]]");
const bridge = new Bridge(fileURLToPath(new URL("./native/target/release/laws-bridge", import.meta.url)));
const results: unknown[] = [];
let failed = false;
try {
  for (const law of isBoundary ? requiredLaws.filter((law) => law.name === "confident-token-nonword-ending-removal") : requiredLaws) {
    const started = performance.now();
    const result = await law.check(bridge, isBoundary ? args[1] : undefined);
    const diagnostic = Arbitrary.formatCheckFailure(result);
    results.push({ law: law.name, seed: law.seed, runs_requested: 256, maxShrinks: 256, seconds: (performance.now() - started) / 1000, result, diagnostic });
    console.log(`${law.name}: ${result._tag}${diagnostic ? "\n" + diagnostic : ""}`);
    failed ||= result._tag !== "Passed";
  }
} finally {
  await bridge.close();
}
await writeFile(new URL(isBoundary ? "./BOUNDARY_RESULT.json" : "./RESULTS.json", import.meta.url), JSON.stringify(results, null, 2) + "\n");
process.exitCode = failed ? 1 : 0;

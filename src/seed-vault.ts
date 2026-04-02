import { setVar } from "./vault.js";

for (const arg of process.argv.slice(2)) {
  const eq = arg.indexOf("=");
  if (eq === -1) {
    console.error(`Skipping invalid argument (no '='): ${arg}`);
    continue;
  }
  const key = arg.slice(0, eq);
  const value = arg.slice(eq + 1);
  await setVar(key, value, "Bootstrap credential", "run.sh");
}

// Deliberately malformed synthetic subprocesses exercise the real transport boundary.
import readline from "node:readline";
const mode = process.argv[2];
if (mode === "exit") process.exit(7);
if (mode === "oversized") process.stdout.write("x".repeat(4097));
for await (const line of readline.createInterface({ input: process.stdin })) {
  const request = JSON.parse(line);
  if (mode === "timeout" || mode === "oversized") continue;
  if (mode === "malformed") { console.log("{bad"); continue; }
  const reply = { id: request.id, hinted: request.text, plain: request.text, breakable: request.text };
  if (mode === "id") reply.id++;
  if (mode === "shape") delete reply.plain;
  if (mode === "unknown") reply.extra = true;
  console.log(JSON.stringify(reply));
}

import assert from "node:assert/strict";
import { after, test } from "node:test";
import { fileURLToPath } from "node:url";
import * as Arbitrary from "effect/Arbitrary";
import type { TextReply } from "./protocol.ts";
import { Bridge } from "./bridge.ts";
import { requiredLaws, tokens } from "./catalog.ts";

const bridge = new Bridge(fileURLToPath(new URL("./native/target/release/laws-bridge", import.meta.url)));
after(async () => await bridge.close());
for (const law of requiredLaws) {
  test(`${law.name}: 256 seeded generated cases`, async () => {
    const result = await law.check(bridge);
    assert.equal(result._tag, "Passed", Arbitrary.formatCheckFailure(result));
    if (result._tag === "Passed") assert.equal(result.runs, 256);
  });
}


test("every token family is recognized and removed for each declared body variant", async () => {
  for (const variant of ["alphanumeric", "mixed", "hyphens", "underscores"] as const) {
    const expected = tokens({ prefix: [], suffix: [], body: Array<"0">(32).fill("0"), extended: Array<"-">(32).fill("-"), variant });
    const reply = await bridge.request({ op: "text", text: expected.join(" ") }) as TextReply;
    for (const token of expected) {
      assert.ok(reply.keys.some((key) => key.confident && key.value === token));
      assert.ok(!reply.patterns.includes(token) && !reply.scrub.includes(token));
    }
  }
});

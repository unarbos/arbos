import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import { Bridge } from "./bridge.ts";

const fixture = fileURLToPath(new URL("./bridge-fixture.mjs", import.meta.url));

test("sequential transport preserves Unicode and concurrent callers' response IDs", async () => {
  const bridge = new Bridge(process.execPath, [fixture, "valid"]);
  try {
    const replies = await Promise.all(["λ😀", "two", "三"].map((text) => bridge.request({ op: "url", text })));
    assert.deepEqual(replies.map((r) => r.id), [0, 1, 2]);
    assert.deepEqual(replies.map((r) => "plain" in r ? r.plain : null), ["λ😀", "two", "三"]);
  } finally {
    await bridge.close();
  }
});

for (const mode of ["exit", "malformed", "id", "shape", "unknown", "oversized", "timeout"]) {
  test(`transport rejects ${mode} and cannot resume as a valid oracle`, async () => {
    const bridge = new Bridge(process.execPath, [fixture, mode], { timeoutMs: 200, maxLineBytes: 4096 });
    try {
      await assert.rejects(bridge.request({ op: "url", text: "synthetic" }));
      await assert.rejects(bridge.request({ op: "url", text: "retry" }));
    } finally {
      await bridge.close();
    }
  });
}

test("request byte bound rejects before writing and leaves the valid process usable", async () => {
  const bridge = new Bridge(process.execPath, [fixture, "valid"]);
  try {
    await assert.rejects(bridge.request({ op: "url", text: "😀".repeat(20000) }), /exceeds limit/);
    const reply = await bridge.request({ op: "url", text: "still valid" });
    assert.equal("plain" in reply ? reply.plain : null, "still valid");
  } finally {
    await bridge.close();
  }
});

test("missing executable rejects without leaking subprocess error details", async () => {
  const bridge = new Bridge("/nonexistent-arbos-synthetic-bridge");
  try {
    await assert.rejects(bridge.request({ op: "url", text: "private synthetic input" }), (error: Error) => {
      assert.doesNotMatch(error.message, /private synthetic|nonexistent-arbos/);
      return true;
    });
  } finally {
    await bridge.close();
  }
});

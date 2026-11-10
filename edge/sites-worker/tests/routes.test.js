import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { resolve } from "../src/index.js";

const fixture = JSON.parse(readFileSync(new URL("./routes.json", import.meta.url), "utf8"));

function site(scheme, labels, path) {
  // fetch() lowercases the hostname before passing its labels to resolve().
  const result = resolve(scheme, labels.toLowerCase(), path);
  if (result.kind !== "site") return null;
  const { prefix, app, rest } = result;
  return { prefix, app, rest };
}

for (const entry of fixture.routes) {
  test(entry.name, () => {
    assert.deepEqual(site(entry.scheme, entry.labels, entry.path), entry.resolved);
  });
}

test("every reserved core name stays outside site routing", () => {
  for (const prefix of fixture.reserved) {
    assert.equal(site("subdomain", prefix, "/notes/"), null, prefix);
    assert.equal(site("two-level", `notes.${prefix}`, "/"), null, prefix);
  }
});

test("Worker landing, slash redirect and invalid-app responses stay distinct", () => {
  assert.deepEqual(resolve("subdomain", "alice", "/"), { kind: "user", prefix: "alice" });
  assert.deepEqual(resolve("subdomain", "alice", "/notes"), {
    kind: "site", prefix: "alice", app: "notes", rest: "", needsSlash: true,
  });
  assert.deepEqual(resolve("subdomain", "alice", "/notes/"), {
    kind: "site", prefix: "alice", app: "notes", rest: "", needsSlash: false,
  });
  assert.deepEqual(resolve("subdomain", "alice", "/Not-a-site/"), { kind: "bad" });
  assert.equal(resolve("path", "alice", "/notes/"), "passthrough");
});

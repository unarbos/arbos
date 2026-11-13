// #2867's merge condition, measured where it was actually broken: the text a `stop_job` answer
// reaches the chat as.
//
// That condition was that an unconfirmed sweep's cause and its skipped session ids reach an
// observer. agentd's `Sweep` carried them correctly and a struct-level test proved it, and the
// fields still reached nobody — `harness/arbos-pi-ext/index.ts` rendered the answer as
// `${states}. ${note}` and dropped `unconfirmed_because`, `sessions_not_read`,
// `processes_checked` and the `unnamed_*` fields before the chat row, the offloaded tool-output
// blob, the relay audit trail or `.arbos/audit.jsonl` could carry any of them.
//
// **So this file asserts on the rendered text and nothing else.** A struct-level test is what let
// the gap through, and one more of those would let the next one through too.
//
// It is driven end to end rather than by calling the renderer: a real loopback HTTP server
// answering `POST /hook/stop_job` with agentd's own JSON, the real `hook()` with a real `fetch`,
// and the real registered `stop_job` tool's `execute`. The only thing not exercised is the model
// choosing to call it.
//
// Its own file because `index.test.ts` runs with `ARBOS_HOOK_URL` unset on purpose — several of
// its cases depend on the hook answering `null` — and that is read once at module load.
//
// Measured against the shipped renderer rather than assumed: with `execute` put back to
// `${states}. ${note}`, three of the four cases below fail. The fourth — the withheld-field
// case — passes either way by construction, because it asserts that a field agentd did not send
// is not printed, and a renderer that prints nothing satisfies that. It is kept as a guard on
// this fix, not as a reproduction of the bug.
import assert from "node:assert/strict";
import { createServer, type Server } from "node:http";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

/** The answer agentd is to give, swapped per case. Serialised exactly as `hooks.rs::ok` does. */
let answer: Record<string, unknown> = {};

const server: Server = createServer((req, res) => {
  const chunks: Buffer[] = [];
  req.on("data", (c) => chunks.push(c));
  req.on("end", () => {
    assert.equal(req.url, "/hook/stop_job", "the tool must call the stop_job hook");
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({ ok: true, result: answer }));
  });
});
await new Promise<void>((done) => server.listen(0, "127.0.0.1", done));
const port = (server.address() as { port: number }).port;

process.env.ARBOS_STATE_DIR = mkdtempSync(join(tmpdir(), "arbos-stop-job-render-"));
process.env.ARBOS_HOOK_URL = `http://127.0.0.1:${port}`;
process.env.ARBOS_SESSION_ROLE = "main";

const { default: arbosPiExt } = await import("./index.ts");

/** The slice of pi's ExtensionAPI registration touches. */
function tools() {
  const registered = new Map<string, any>();
  arbosPiExt({
    on: () => () => {},
    registerTool: (t: any) => registered.set(t.name, t),
    appendEntry: () => {},
  } as any);
  return registered;
}

/** The text the chat gets, through the registered tool and the real hook. */
async function rendered(from: Record<string, unknown>): Promise<string> {
  answer = from;
  const res = await tools().get("stop_job").execute("call-1", { job_id: "all" });
  return res.content[0].text as string;
}

/** Forest's G2 cancel, with the sweep unable to read one session (`UnreadCause::AdapterLockContended`). */
const UNCONFIRMED = {
  stopped: [
    {
      job_id: "01M6RK3DWRBT3R0YKBJ5ET2QD3",
      title: "Writing survivor-a step log",
      state: "cancelled",
      processes_checked: false,
      processes_killed: 0,
      processes_still_alive: [],
    },
  ],
  asks_cancelled: 0,
  all_cancelled: false,
  unnamed_processes_checked: false,
  unnamed_processes_killed: 0,
  unnamed_processes_still_alive: [],
  unconfirmed_because: "adapter_lock_contended",
  sessions_not_read: ["01M6RK5PJWGZ337K7CXE1EE8QB"],
  note: "Every job above reads `cancelled`, but its processes could NOT be checked.",
};

test("an unconfirmed cancel's cause and skipped sessions reach the chat text (#2867)", async () => {
  const text = await rendered(UNCONFIRMED);

  // The two keys the merge condition named. Both were dropped here, and a canary reading the
  // chat could not tell this answer from a clean one.
  assert.match(text, /adapter_lock_contended/, text);
  assert.match(text, /01M6RK5PJWGZ337K7CXE1EE8QB/, text);

  // And the evidence beside them, which decides whether the box may claim a kill at all.
  assert.match(text, /processes checked no/, text);
  assert.match(text, /Work belonging to no job row: checked no/, text);

  // What the renderer already carried must survive the fix.
  assert.match(text, /Writing survivor-a step log — state cancelled/, text);
  assert.ok(text.includes(UNCONFIRMED.note), text);
});

test("a confirmed cancel says nothing about an unread part (the negative control)", async () => {
  // Without this, a renderer that printed "Unconfirmed because" unconditionally would pass the
  // case above — and an all-clear answer that reads as unconfirmed is the opposite dishonesty.
  const text = await rendered({
    stopped: [
      {
        job_id: "01M6RK3DWRBT3R0YKBJ5ET2QD3",
        title: "Writing survivor-a step log",
        state: "cancelled",
        processes_checked: true,
        processes_killed: 5,
        processes_still_alive: [],
      },
    ],
    all_cancelled: true,
    unnamed_processes_checked: true,
    unnamed_processes_killed: 0,
    unnamed_processes_still_alive: [],
    note: "Every job above reads `cancelled` in its own row, and every process it had started was checked and is dead.",
  });

  assert.doesNotMatch(text, /Unconfirmed because/, text);
  assert.doesNotMatch(text, /sessions not read/, text);
  assert.match(text, /processes checked yes, killed 5, still alive none/, text);
});

test("a field agentd withheld is not rendered as a value (bug AE's shape)", async () => {
  // `Sweep::checked` answers `None` for a job that never had a worker, and `hook_stop_job` then
  // omits `processes_checked` rather than sending a convenient `true`. A renderer that filled the
  // gap in either direction would undo that: `no` invents a failure, `yes` is bug AE's sentence
  // with the numbers filled in.
  const text = await rendered({
    stopped: [{ job_id: "01M6RK3DWRBT3R0YKBJ5ET2QD3", title: "Counting", state: "cancelled" }],
    all_cancelled: true,
    note: "Every job above reads `cancelled` in its own row, and none of them had a worker running.",
  });

  assert.doesNotMatch(text, /checked/, text);
  assert.doesNotMatch(text, /killed/, text);
  assert.match(text, /Counting — state cancelled\./, text);
});

test("a cancelled row-less ask still carries the unread part it was answered with", async () => {
  // The `stopped.is_empty()` path: no job row, so no per-job sweep to print, and the box-wide
  // reap's unread part is the only evidence there is.
  const text = await rendered({
    stopped: [],
    asks_cancelled: 1,
    all_cancelled: true,
    unnamed_processes_still_alive: [],
    unconfirmed_because: "proc_unreadable",
    note: "No job was running. This chat had asks that owned no job row.",
  });

  assert.match(text, /nothing\./, text);
  assert.match(text, /Unconfirmed because: proc_unreadable\./, text);
  // `proc_unreadable` stops the walk before it can name a session, so there are none to name
  // and the clause must not trail an empty list.
  assert.doesNotMatch(text, /sessions not read/, text);
});

test.after(() => server.close());

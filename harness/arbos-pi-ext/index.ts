/**
 * arbos-pi-ext: the one pi extension Arbos loads into the main agent (and the
 * pi fallback subagent). Design D64/D65/D66, arbos-design.md §4.4.1.
 *
 * 1. `tool_call` hook: deny-list veto + audit append BEFORE execution.
 * 2. `ask_user(question, widget?)` tool → agentd raises InputRequested (S8) or a typed question card.
 * 3. `route` custom tool, validated by agentd (one retry, then `reply`).
 * 4. `tool_result` offload (> 4,000 chars → blob + handle), `peek`, working-set section.
 *
 * Everything that needs agentd goes over the loopback hook API (ARBOS_HOOK_URL).
 * The extension keeps working with no agentd (local pi runs): hooks degrade to no-ops.
 */

import { appendFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { join } from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

import { DENY_RULES, matchDeny } from "./deny.ts";
import { ROUTE_PARAMS, type RouteArgs } from "./route-schema.ts";
import { describeWidget, WIDGET_PARAM } from "./widget-schema.ts";

const STATE_DIR = process.env.ARBOS_STATE_DIR ?? join(process.env.HOME ?? "/tmp", ".arbos");
const AUDIT_PATH = process.env.ARBOS_AUDIT_PATH ?? join(STATE_DIR, "audit.jsonl");
const BLOB_DIR = process.env.ARBOS_BLOB_DIR ?? join(STATE_DIR, "blobs");
const HOOK_URL = process.env.ARBOS_HOOK_URL ?? "";
const HOOK_TOKEN = process.env.ARBOS_MCP_TOKEN ?? "";
const SESSION_ROLE = process.env.ARBOS_SESSION_ROLE ?? "main"; // main | subagent
const OFFLOAD_CHARS = Number(process.env.ARBOS_OFFLOAD_CHARS ?? 4000);
const HEAD_LINES = 20;
const TAIL_LINES = 10;
const ROUTE_PREFIX = "(Arbos: call `route` first.) ";
// Mirrors `agent::STEER_PREFIX`: a user steer agentd stamped (#1690). It lands inside a routed
// turn — as pi's steer, or as the prompt that continues the turn after agentd aborted a long
// tool call — so it gets no route reminder and the turn keeps its `routed` state.
const STEER_PREFIX = "[Arbos steer:";
// Mirrors `agent::RELAY_TURN_MARK`: agentd stamps every relay turn (a job's or subagent's
// result, an approval outcome, an agent's reply) — turns whose only valid route is `reply`.
const RELAY_TURN_MARK = "[Arbos relay turn]";

/**
 * A shell command that would outlive the turn (nohup / setsid / disown / a trailing `&` / tmux /
 * screen / systemd-run) in the main agent's own turn. That work is a job: `start_job` runs it on a
 * worker with status, steer_job and stop_job; a detached script has none of that, and a later
 * change becomes kill + restart (#1778, b9: `kill …; pkill -f 'sleep 45'` raised a destructive
 * card). Returns the offending fragment, or null.
 */
export function detachedShell(toolName: string, input: Record<string, unknown>): string | null {
  const text =
    toolName === "bash" ? String(input?.command ?? "")
    : toolName === "codemode" ? String(input?.code ?? "")
    : "";
  if (!text) return null;
  const patterns: RegExp[] = [
    /(^|[;&|(`'"]\s*|\bsudo\s+|\bexec\s+)(nohup|setsid)\b/m,
    /\bdisown\b/,
    /\b(tmux\s+new(-session)?|screen\s+-[dD]|systemd-run|start-stop-daemon|at\s+now)\b/,
    // `cmd &` / `(cmd) &` / `cmd & disown` / `cmd &\n…` — not `&&`, not `2>&1`, not `&>`.
    /(?<![&<>|\d])&(?![&>])\s*(?:$|[;)\n]|\n|disown\b|\\n)/m,
  ];
  for (const re of patterns) {
    const m = re.exec(text);
    if (m) return text.slice(Math.max(0, m.index - 40), m.index + m[0].length + 40).replace(/\s+/g, " ").trim();
  }
  return null;
}

const DETACHED_SHELL_HINT =
  "Detached processes are not started from a turn (nohup, setsid, disown, a trailing `&`, tmux, screen): that work is a job. Call start_job(title, brief, kind) with the whole instruction — every step, the waits, the exact file paths, what to report — and end the turn; the worker runs it with a status line, its result arrives as an [Arbos] job notice, and a later change goes through steer_job(job_id, text) (never kill + restart). Run short foreground commands here only.";

/**
 * A `reply` text that announces more to come ("delivered in parts below", "table follows") is an
 * introduction, not the answer: the turn must go on with send_message. Mirrors
 * `agent::promises_parts` (proof run 1, 2026-10-26 01:0x UTC: the result turn routed `reply`
 * with that sentence, the route tool ended the turn, and no part ever followed).
 */
export function promisesMore(text: string): boolean {
  const t = text.toLowerCase();
  return /\b(in parts?|parts? (below|follow|to follow|coming)|(details?|table|comparison|list|sources|rest|more)\s+(below|follows?|to follow|coming|next)|below[.:!]?\s*$|coming (up|next|right up)|next message|follows? (shortly|now|below)|here (it|they) (come|are)s?:?\s*$)\b/.test(t)
    || /\bbelow\b/.test(t) && t.length < 160;
}
// Mirrors `arbos_core::approval::SHELL_TOOLS`; the classification itself happens in agentd.
const SHELL_TOOLS = new Set(["bash", "shell", "run_command", "execute_command"]);
const APPROVAL_POLL_MS = Number(process.env.ARBOS_APPROVAL_POLL_MS ?? 1500);
const APPROVAL_WAIT_MS = Number(process.env.ARBOS_APPROVAL_WAIT_MS ?? 14 * 60 * 1000); // under the turn deadline
// Code mode (D86): agentd sets ARBOS_CODE_MODE=on|off per pi spawn (flag file / box env, default off).
const CODE_MODE = (process.env.ARBOS_CODE_MODE ?? "off").trim().toLowerCase() === "on";
// Default deadline of a `codemode` script when the model sets none (code mode on): the turn deadline.
const SCRIPT_TIMEOUT_MS = Number(process.env.ARBOS_SCRIPT_TIMEOUT_MS ?? 15 * 60 * 1000);
// Tools that talk to the user or to agentd's turn state: direct calls only, never from a script.
const DIRECT_ONLY_TOOLS = new Set(["route", "ask_user", "send_message", "start_job", "list_jobs", "steer_job", "stop_job", "mcp__arbos__ask_user"]);
// pi's built-in tools inside a `codemode` script: these keep their plain `tools.<name>`.
const PI_BUILTIN_TOOLS = new Set(["read", "bash", "edit", "write", "grep", "find", "ls", "codemode"]);
// Stable prefix (D87): with it on, the working set, the per-turn skills and project memory are
// not system-prompt sections (which rewrite the cached prefix whenever they change) but a block
// inside the user turn, so the prefix — tools, system prompt, earlier turns — stays byte-stable.
const STABLE_PREFIX = (process.env.ARBOS_STABLE_PREFIX ?? "off").trim().toLowerCase() === "on";
const CONTEXT_OPEN = "[Arbos context for this turn — working set, skills, project memory. Not from the user.]";
const CONTEXT_CLOSE = "[End of Arbos context]";

type ApprovalState = "allowed" | "pending" | "approved" | "rejected" | "expired";

type HookResponse = { ok: boolean; error?: string; terminate?: boolean; text?: string; [k: string]: unknown };

async function hook(path: string, body: unknown): Promise<HookResponse | null> {
  if (!HOOK_URL) return null;
  try {
    const res = await fetch(`${HOOK_URL}${path}`, {
      method: "POST",
      headers: { "content-type": "application/json", authorization: `Bearer ${HOOK_TOKEN}` },
      body: JSON.stringify(body),
    });
    return (await res.json()) as HookResponse;
  } catch (err) {
    return { ok: false, error: `agentd unreachable: ${String(err)}` };
  }
}

function ensureDir(dir: string): void {
  if (!existsSync(dir)) mkdirSync(dir, { recursive: true });
}

function audit(entry: Record<string, unknown>): void {
  try {
    ensureDir(STATE_DIR);
    appendFileSync(AUDIT_PATH, `${JSON.stringify({ ts: Date.now(), ...entry })}\n`);
  } catch {
    // The audit file is best-effort on a read-only test host; the hook API mirrors it.
  }
}

function inputHash(input: unknown): string {
  return createHash("sha256").update(JSON.stringify(input ?? null)).digest("hex").slice(0, 16);
}

interface RouteDetails {
  action?: string;
  fallback?: string;
  error?: string;
}

interface Handle {
  id: string;
  kind: string;
  chars: number;
  lines: number;
  path: string;
  note: string;
  createdAt: number;
}

export default function arbosPiExt(pi: ExtensionAPI) {
  // Per-turn state. Reset at every run start.
  let routeAttempts = 0;
  let routed = false;
  // A relay turn (a job's or subagent's result, an approval outcome, an agent's reply): its only
  // valid route is `reply`, so a `send_message` that comes first is that route (sweep 2026-10-25:
  // the result turn's two send_message calls were blocked "route-first"; the model then routed
  // `reply` with "Delivering the update now." and the user read only that).
  let relayTurn = false;
  let routeCorrections = 0;
  let correcting = false;
  let sessionId = "";
  const handles = new Map<string, Handle>();
  const scratch = new Map<string, string>();

  pi.on("session_start", async (_event, ctx) => {
    sessionId = ctx.sessionManager.getSessionId?.() ?? "";
    lastContextSent = "";
    for (const entry of ctx.sessionManager.getBranch()) {
      if (entry.type === "custom" && entry.customType === "arbos-handle") {
        const h = entry.data as Handle;
        handles.set(h.id, h);
      }
      if (entry.type === "custom" && entry.customType === "arbos-remember") {
        const d = entry.data as { name: string; value: string | null };
        if (d.value === null) scratch.delete(d.name);
        else scratch.set(d.name, d.value);
      }
    }
  });

  /** The working-set lines of this chat (handles, notes, agentd's running subagents / goals / summary). */
  async function workingSet(): Promise<{ lines: string[]; skills: string; memory: string }> {
    const lines: string[] = [];
    for (const h of handles.values()) {
      lines.push(`- ${h.id} (${h.kind}, ${h.chars.toLocaleString()} chars, ${h.lines} lines): ${h.note}`);
    }
    for (const [k, v] of scratch) {
      lines.push(`- remember:${k} = ${v.length > 120 ? `${v.slice(0, 117)}…` : v}`);
    }
    const remote = await hook("/hook/working-set", { session_id: sessionId, role: SESSION_ROLE });
    if (remote?.ok && typeof remote.text === "string" && remote.text.trim()) lines.push(remote.text.trim());
    const skills = remote?.ok && typeof remote.skills === "string" ? remote.skills.trim() : "";
    const memory = remote?.ok && typeof remote.memory === "string" ? remote.memory.trim() : "";
    return { lines: lines.slice(0, 40), skills, memory }; // ≈ 600 tokens of lines (design §6.6 item 2b)
  }

  // ---- 4b. working-set section, refreshed every run and after compaction -----------------
  pi.on("before_agent_start", async (event) => {
    routeAttempts = 0;
    // #1704: a run whose prompt is a stamped steer continues the turn agentd interrupted (#1690);
    // that turn already routed, so the route-first guard does not apply again. Read from the
    // prompt itself: state carried from the `input` hook can miss when the hooks run apart.
    routed = event.messageType === "user" && event.prompt.startsWith(STEER_PREFIX);
    // The `input` hook prefixed the prompt (route reminder, turn context): the mark is inside.
    relayTurn = event.messageType === "user" && event.prompt.includes(RELAY_TURN_MARK);
    routeCorrections = 0;
    correcting = false;
    if (STABLE_PREFIX) return; // D87: the context rides in the user turn (see the `input` hook)
    const ws = await workingSet();
    if (ws.lines.length > 0) {
      event.systemPromptOptions.sections["arbos-working-set"] =
        `Named handles and notes of this chat. Use peek(handle) for slices; never ask to re-run them.\n${ws.lines.join("\n")}`;
    }
    // Relevant skills (F5): a per-turn section after the byte-stable prefix, ≤ 600 tokens, chosen by agentd.
    if (ws.skills) {
      event.systemPromptOptions.sections["arbos-skills"] = ws.skills;
    }
  });

  /**
   * D87: the per-turn context as one block placed before the user's words in the user turn — and
   * only when it changed since the last turn of this session (append-only: an unchanged working set
   * is already in the cached history; a new handle or note re-sends the small block once).
   */
  let lastContextSent = "";
  async function turnContextBlock(): Promise<string> {
    const ws = await workingSet();
    const parts: string[] = [];
    if (ws.lines.length > 0) {
      parts.push(`Working set — named handles and notes of this chat; use peek(handle) for slices, never ask to re-run them:\n${ws.lines.join("\n")}`);
    }
    if (ws.skills) parts.push(ws.skills);
    if (ws.memory) parts.push(`Project memory:\n${ws.memory}`);
    const block = parts.length === 0 ? "" : `${CONTEXT_OPEN}\n${parts.join("\n\n")}\n${CONTEXT_CLOSE}\n\n`;
    if (block === lastContextSent) return "";
    lastContextSent = block;
    return block;
  }

  // ---- 1. deny-list veto + audit before execution ----------------------------------------
  // Calls a `codemode` script makes arrive here too (pi runs nested calls through the same
  // pipeline, with `parentToolCallId` set): deny-list, audit and approval apply to them unchanged.
  pi.on("tool_call", async (event) => {
    const parent = event.parentToolCallId ?? null;
    const denied = matchDeny(event.toolName, event.input as Record<string, unknown>);
    const directOnly = parent !== null && DIRECT_ONLY_TOOLS.has(event.toolName);
    // Agent-flow §1: the main turn must `route` (with `ack` for act/new/wake) before any other
    // tool. Without this, the chat shows only "<name> is working" while work runs.
    const beforeRoute =
      SESSION_ROLE === "main" && !routed && event.toolName !== "route" && parent === null;
    audit({
      kind: "tool_call",
      session: sessionId,
      role: SESSION_ROLE,
      tool: event.toolName,
      call_id: event.toolCallId,
      parent,
      input_hash: inputHash(event.input),
      input_preview: JSON.stringify(event.input).slice(0, 200),
      denied:
        denied?.rule ??
        (directOnly ? "direct-only-from-script" : null) ??
        (beforeRoute ? "route-first" : null),
    });
    if (denied) {
      void hook("/hook/deny", { session_id: sessionId, tool: event.toolName, rule: denied.rule, call_id: event.toolCallId });
      return { block: true, reason: `Blocked by the Arbos deny-list (${denied.rule}): ${denied.why}` };
    }
    if (directOnly) {
      return { block: true, reason: `${event.toolName} cannot be called from a script: call it directly, after the script.` };
    }
    if (SESSION_ROLE === "main" && parent === null) {
      const detached = detachedShell(event.toolName, event.input as Record<string, unknown>);
      if (detached) {
        audit({ kind: "detached_shell_refused", session: sessionId, tool: event.toolName, fragment: detached.slice(0, 160) });
        return { block: true, reason: `Refused (\`${detached.slice(0, 80)}\`): ${DETACHED_SHELL_HINT}` };
      }
    }
    if (beforeRoute && relayTurn && event.toolName === "send_message") {
      // Speaking first in a relay turn is routing `reply`: record it and let the message out.
      routeAttempts += 1;
      const res = await hook("/hook/route", { session_id: sessionId, role: SESSION_ROLE, attempt: routeAttempts, args: { action: "reply" } });
      routed = true;
      audit({ kind: "route_implicit", session: sessionId, role: SESSION_ROLE, ok: !res || res.ok !== false });
    } else if (beforeRoute) {
      return {
        block: true,
        reason:
          "Call `route` first (required every turn). For act/new/wake, set `ack` to one short sentence the user sees before any work runs.",
      };
    }
    if (event.toolName === "codemode" && parent === null) {
      aliasArbosToolNames(event.input as { code?: unknown });
      ensureScriptDeadline(event.input as { code?: unknown });
    }
    // Risky shell commands wait for an approval card (feature `approve`); agentd's
    // `arbos_core::approval` matcher is the single source of truth for what counts as risky.
    if (SHELL_TOOLS.has(event.toolName)) {
      const verdict = await awaitApproval(event.toolName, event.input, event.toolCallId);
      if (verdict) return verdict;
    }
    return undefined;
  });

  /**
   * The model's prior is `tools.web_fetch(...)`; pi exposes every arbos MCP tool as
   * `tools.mcp__arbos__web_fetch`. Rewrite the plain names (and the `tools.mcp__arbos.name`
   * guess) to the real ones before the script runs, so a turn does not burn one or two errored
   * calls on "tools.web_fetch does not exist. Did you mean …" (#1358). pi's own tools keep their
   * names; anything already prefixed is left alone.
   */
  function aliasArbosToolNames(input: { code?: unknown }): void {
    if (typeof input.code !== "string") return;
    input.code = input.code
      .replace(/\btools\.mcp__arbos\.([A-Za-z_][A-Za-z0-9_]*)/g, "tools.mcp__arbos__$1")
      // the opposite guess: a pi built-in with the arbos prefix ("tools.mcp__arbos__write")
      .replace(/\btools\.mcp__arbos__([a-z]+)\b/g, (whole, name: string) => (PI_BUILTIN_TOOLS.has(name) ? `tools.${name}` : whole))
      .replace(/\btools\.([a-z][a-z0-9_]*)(?=\s*\()/g, (whole, name: string) =>
        PI_BUILTIN_TOOLS.has(name) || name.startsWith("mcp__") ? whole : `tools.mcp__arbos__${name}`,
      );
  }

  /** Code mode on: a script without its own `// @options:` deadline gets the default one. */
  function ensureScriptDeadline(input: { code?: unknown }): void {
    if (!CODE_MODE || typeof input.code !== "string") return;
    const nl = input.code.indexOf("\n");
    const first = nl < 0 ? input.code : input.code.slice(0, nl);
    const m = /^\s*\/\/\s*@options:\s*(\{.*\})\s*$/.exec(first);
    if (m) {
      try {
        const opts = JSON.parse(m[1]) as Record<string, unknown>;
        if (typeof opts.timeout_ms === "number") return;
        opts.timeout_ms = SCRIPT_TIMEOUT_MS;
        input.code = `// @options: ${JSON.stringify(opts)}${nl < 0 ? "" : input.code.slice(nl)}`;
      } catch {
        // Malformed options line: pi reports it to the model; nothing to add.
      }
      return;
    }
    input.code = `// @options: ${JSON.stringify({ timeout_ms: SCRIPT_TIMEOUT_MS })}\n${input.code}`;
  }

  async function awaitApproval(
    tool: string,
    input: unknown,
    callId: string,
  ): Promise<{ block: true; reason: string } | undefined> {
    const first = await hook("/hook/approval", { tool, input, session_id: sessionId, role: SESSION_ROLE });
    if (!first) return undefined; // no agentd: local pi runs are not gated
    if (!first.ok) {
      audit({ kind: "approval_error", session: sessionId, tool, call_id: callId, error: first.error ?? null });
      return undefined;
    }
    let state = String(first.state ?? "allowed") as ApprovalState;
    if (state === "allowed") return undefined;
    const approvalId = String(first.approvalId ?? "");
    audit({ kind: "approval_wait", session: sessionId, tool, call_id: callId, approval_id: approvalId, title: first.title ?? null });
    const deadline = Date.now() + APPROVAL_WAIT_MS;
    let reason: string | undefined;
    while (state === "pending" && Date.now() < deadline) {
      await new Promise((r) => setTimeout(r, APPROVAL_POLL_MS));
      const cur = await hook("/hook/approval-status", { approvalId });
      if (cur?.ok && typeof cur.state === "string") {
        state = cur.state as ApprovalState;
        if (typeof cur.reason === "string") reason = cur.reason;
      }
    }
    if (state === "pending") {
      // Stopped waiting: agentd expires the card so the UI shows the timeout promptly.
      void hook("/hook/approval-abandon", { approvalId });
    }
    audit({ kind: "approval_result", session: sessionId, tool, call_id: callId, approval_id: approvalId, state });
    switch (state) {
      case "allowed":
      case "approved":
        return undefined;
      case "rejected":
        return {
          block: true,
          reason: `The user rejected this action (approval ${approvalId})${reason ? `: ${reason}` : ""}. Do not retry it unchanged; tell the user.`,
        };
      case "expired":
      case "pending":
        return {
          block: true,
          reason: `The approval for this action (${approvalId}) was not decided in time. Nothing ran and nothing will run or "confirm" later: whatever it was meant to post, send or change is NOT done. Tell the user it needs their approval; never say it was posted or sent.`,
        };
      default: {
        const _exhaustive: never = state;
        return { block: true, reason: `Unknown approval state ${String(_exhaustive)}` };
      }
    }
  }

  // ---- 4a. offload hook: big tool results become handles -----------------------------------
  pi.on("tool_result", async (event) => {
    // A result going back into a script stays whole: the script filters it, and only the script's
    // own (capped) output reaches the model — that output is offloaded here like any other result.
    if (event.parentToolCallId) return undefined;
    const textBlocks = event.content.filter((c) => c.type === "text") as { type: "text"; text: string }[];
    const full = textBlocks.map((b) => b.text).join("\n");
    if (full.length <= OFFLOAD_CHARS) return undefined;
    ensureDir(BLOB_DIR);
    const sha = createHash("sha256").update(full).digest("hex");
    const id = `h_${sha.slice(0, 8)}`;
    const path = join(BLOB_DIR, sha);
    if (!existsSync(path)) writeFileSync(path, full);
    const allLines = full.split("\n");
    const handle: Handle = {
      id,
      kind: `${event.toolName} output`,
      chars: full.length,
      lines: allLines.length,
      path,
      note: `${event.toolName} ${JSON.stringify(event.input).slice(0, 80)}`,
      createdAt: Date.now(),
    };
    handles.set(id, handle);
    pi.appendEntry("arbos-handle", handle);
    void hook("/hook/handle", { session_id: sessionId, handle });
    const head = allLines.slice(0, HEAD_LINES).join("\n");
    const tail = allLines.length > HEAD_LINES + TAIL_LINES ? allLines.slice(-TAIL_LINES).join("\n") : "";
    const text =
      `[handle ${id}: ${handle.kind}, ${full.length.toLocaleString()} chars, ${allLines.length.toLocaleString()} lines]\n` +
      `${head}\n${tail ? `…\n${tail}\n` : ""}` +
      `[use peek("${id}", {lines:[from,to]} | {grep:"pattern"}) to read more; do not re-run the command]`;
    return { content: [{ type: "text", text }], details: { arbosHandle: id } };
  });

  // ---- 3. route (main session only: a pi subagent's `route` would hit the main turn's state) ----
  if (SESSION_ROLE === "main") pi.registerTool({
    name: "route",
    label: "Route",
    description: "Required first call of every turn: reply | act | wake | new (see the system prompt).",
    promptSnippet: "route: the required first call of every turn",
    parameters: ROUTE_PARAMS,
    exposure: "model-only",
    executionMode: "sequential",
    async execute(_toolCallId, params): Promise<{ content: { type: "text"; text: string }[]; details: RouteDetails; terminate?: boolean }> {
      const args = params as RouteArgs;
      routeAttempts += 1;
      const res = await hook("/hook/route", { session_id: sessionId, role: SESSION_ROLE, attempt: routeAttempts, args });
      if (res && !res.ok) {
        if (routeAttempts >= 2) {
          routed = true;
          return {
            content: [{ type: "text", text: `Route invalid twice (${res.error ?? "unknown"}). Arbos treats this turn as reply. Write your answer now.` }],
            details: { fallback: "reply", error: res.error },
          };
        }
        throw new Error(`Invalid route: ${res.error ?? "unknown"}. Call route again with corrected arguments.`);
      }
      routed = true;
      const action = args.action;
      if (correcting) {
        // Post-hoc route after a reply that skipped it: record it, do not answer again.
        correcting = false;
        void hook("/hook/route-corrected", { session_id: sessionId, action });
        return { content: [{ type: "text", text: "Route recorded. Do not repeat your answer; end the turn." }], details: { action }, terminate: true };
      }
      const acked = typeof args.ack === "string" && args.ack.trim().length > 0;
      switch (action) {
        case "reply":
          if (args.reply && args.reply.trim()) {
            if (relayTurn || promisesMore(args.reply)) {
              // An opening line, not the answer: the turn goes on until the parts are posted.
              return {
                content: [{ type: "text", text: "Your first line is in the chat. It is not the answer: now post the rest with send_message — each part its own call (conclusion, linked table, details + sources, one next-step offer) — then end the turn. Words outside send_message are not shown." }],
                details: { action },
              };
            }
            return { content: [{ type: "text", text: "Reply delivered to the user." }], details: { action }, terminate: true };
          }
          return { content: [{ type: "text", text: "Route accepted: reply. Post the answer with send_message (one call for a short answer; parts for a comparison); words outside send_message are not shown." }], details: { action } };
        case "act":
          return {
            content: [
              {
                type: "text",
                text: acked
                  ? "Route accepted: act. Your acknowledgement is on the working line. If this needs the web, several pages, a build, or more than three tool calls: post that acknowledgement with send_message, call start_job now and end the turn — do not do long work here. Otherwise do the work, then post the result with send_message (a comparison as conclusion, linked table, details + sources, one next-step offer — each its own call); words outside send_message are not shown. No thinking aloud, no process narration. A destructive step (deleting files you did not create, sending, paying) still needs the user's yes through ask_user before you run it."
                  : "Route accepted: act. If this needs the web, several pages, a build, or more than three tool calls: post a one-line acknowledgement with send_message, call start_job now and end the turn. Otherwise follow your plan, then post the result with send_message (several calls when the answer has parts); words outside send_message are not shown. No thinking aloud, no process narration. A destructive step (deleting files you did not create, sending, paying) still needs the user's yes through ask_user before you run it.",
              },
            ],
            details: { action },
          };
        case "wake":
        case "new":
          if (acked) {
            // The `ack` sentence is already in the chat as its own bubble: nothing more to say now.
            return {
              content: [{ type: "text", text: `Route accepted: ${action}. Your acknowledgement is in the chat; the result arrives later as an [Arbos] message. End the turn now without writing anything.` }],
              details: { action },
              terminate: true,
            };
          }
          // No terminate: the model writes the one-sentence acknowledgement itself (demo review item 2).
          return {
            content: [
              {
                type: "text",
                text: `Route accepted: ${action}. Arbos runs the work in the background and will hand you its result as a later [Arbos] message. Now write one plain sentence to the user saying what you are getting done (no subagent name, no harness, no model, no tool talk), then end the turn. No list, no JSON, no questions.`,
              },
            ],
            details: { action },
          };
        default: {
          const never: never = action;
          throw new Error(`unknown action ${String(never)}`);
        }
      }
    },
  });

  // ---- 2b. send_message: progress bubble (no `to`) or cross-subnet agent message (`to`) ------
  if (SESSION_ROLE === "main") pi.registerTool({
    name: "send_message",
    label: "Send message",
    description:
      "Two uses. (1) Without `to`: post a message to the user in this chat — the only way your words reach them (an acknowledgement before longer work, a progress note at a real milestone, each part of a result, or the whole answer). Markdown is fine: tables, lists, links. (2) With `to` (agent name, @handle or id): message that agent — the text lands in their own chat and their reply comes back here; never write `@name …` as a chat bubble to reach them. Optional `activity`: 2–5 plain words for the working line (\"Running tests\"). Optional `reply_to`: the id of the message this answers (default: the message that started this turn).",
    promptSnippet: "send_message: post to the user (the only way your words are shown); with `to` = message another agent",
    parameters: Type.Object({
      text: Type.String({ description: "The message for the user (markdown allowed), or the words for the other agent" }),
      to: Type.Optional(Type.Union([Type.String(), Type.Null()], { description: "Agent name, @handle or id — set this to message another agent" })),
      activity: Type.Optional(Type.Union([Type.String(), Type.Null()], { description: "What you are doing next, 2-5 words, present tense" })),
      reply_to: Type.Optional(Type.Union([Type.String(), Type.Null()], { description: "Message id this answers, when not the message that started the turn" })),
    }),
    exposure: "model-only",
    executionMode: "sequential",
    async execute(_toolCallId, params) {
      const res = await hook("/hook/send_message", {
        session_id: sessionId,
        text: params.text,
        to: params.to ?? null,
        activity: params.activity ?? null,
        reply_to: params.reply_to ?? null,
      });
      if (res && !res.ok) {
        return { content: [{ type: "text", text: `Not sent (${res.error ?? "unknown"}). Keep working; say it in your final result instead.` }], details: undefined };
      }
      if (typeof res?.note === "string" && res.note.trim()) {
        return { content: [{ type: "text", text: res.note }], details: undefined };
      }
      return { content: [{ type: "text", text: "Posted. Do not repeat it; anything you write outside send_message is not shown." }], details: undefined };
    },
  });

  // ---- 2c. jobs (duplex step 2): start_job returns at once; the result comes back as an [Arbos] job notice
  if (SESSION_ROLE === "main") pi.registerTool({
    name: "start_job",
    label: "Start job",
    description:
      "Start a background job for work that takes more than a few seconds (research, a comparison, a build, a long script). Returns at once with a job id; the job's title is the status line the user watches, and its result reaches you later as an `[Arbos] Job … finished` message to deliver with send_message. Post a one-line acknowledgement with send_message first, then call this and end the turn.",
    promptSnippet: "start_job: run longer work beside the chat; its result arrives later as an [Arbos] job notice",
    parameters: Type.Object({
      title: Type.String({ description: "2-5 words, present tense, what the user sees while it runs (\"Searching for surf schools\")" }),
      brief: Type.String({ description: "The exact instruction for the worker: what to find or build, for whom, the acceptance criterion, what to return (URLs and sources for research)" }),
      kind: Type.Union([Type.Literal("research"), Type.Literal("code"), Type.Literal("ops"), Type.Literal("other")]),
      touches: Type.Optional(Type.Union([Type.Array(Type.String()), Type.Null()], { description: "Resources the job changes: paths, 'git', 'routines', 'secret:<name>' (none for research)" })),
    }),
    exposure: "model-only",
    executionMode: "sequential",
    async execute(_toolCallId, params) {
      const res = await hook("/hook/start_job", {
        session_id: sessionId,
        title: params.title,
        brief: params.brief,
        kind: params.kind,
        touches: params.touches ?? [],
      });
      if (res && !res.ok) {
        return { content: [{ type: "text", text: `Job not started (${res.error ?? "unknown"}). Tell the user in one line what you will do instead.` }], details: undefined };
      }
      const job = (res as { job?: { job_id?: string; state?: string } } | undefined)?.job;
      return {
        content: [{ type: "text", text: `Job ${job?.job_id ?? "?"} ${job?.state ?? "running"}. End the turn now with nothing more; its result arrives later as an [Arbos] job notice.` }],
        details: undefined,
      };
    },
  });

  if (SESSION_ROLE === "main") pi.registerTool({
    name: "list_jobs",
    label: "List jobs",
    description: "The jobs of this chat: id, title, state (queued | running | done | failed | cancelled | interrupted), current step and elapsed seconds. Use it to answer \"how is it going?\" and before starting work that may already be running.",
    promptSnippet: "list_jobs: the jobs of this chat with their live step",
    parameters: Type.Object({}),
    exposure: "model-only",
    executionMode: "sequential",
    async execute() {
      const res = await hook("/hook/list_jobs", { session_id: sessionId });
      if (res && !res.ok) {
        return { content: [{ type: "text", text: `No job list (${res.error ?? "unknown"}).` }], details: undefined };
      }
      const jobs = (res as { jobs?: unknown[] } | undefined)?.jobs ?? [];
      return { content: [{ type: "text", text: jobs.length ? JSON.stringify(jobs) : "No jobs in this chat." }], details: undefined };
    },
  });

  if (SESSION_ROLE === "main") pi.registerTool({
    name: "steer_job",
    label: "Steer job",
    description: "Send a correction into a running job (\"only group lessons under $70\"). `job_id` from list_jobs, or \"latest\" for the newest running job. Ack to the user yourself in one line.",
    promptSnippet: "steer_job: a mid-flight correction to a running job",
    parameters: Type.Object({
      job_id: Type.String({ description: "Job id, or \"latest\"" }),
      text: Type.String({ description: "The correction, as an instruction to the worker" }),
    }),
    exposure: "model-only",
    executionMode: "sequential",
    async execute(_toolCallId, params) {
      const res = await hook("/hook/steer_job", { session_id: sessionId, job_id: params.job_id, text: params.text });
      if (res && !res.ok) {
        return { content: [{ type: "text", text: `Not steered (${res.error ?? "unknown"}). Start a new job for it, or tell the user.` }], details: undefined };
      }
      const job = (res as { job?: { job_id?: string; title?: string } } | undefined)?.job;
      return { content: [{ type: "text", text: `Steered job ${job?.job_id ?? "?"} (${job?.title ?? ""}). Tell the user in one line what changed, then end the turn.` }], details: undefined };
    },
  });

  if (SESSION_ROLE === "main") pi.registerTool({
    name: "stop_job",
    label: "Stop job",
    description: "Stop a running job: `job_id` from list_jobs, \"latest\" for the newest running job, or \"all\" for every job of this chat. Confirm to the user yourself in one line; no result will follow.",
    promptSnippet: "stop_job: stop one job (id | latest) or all",
    parameters: Type.Object({
      job_id: Type.String({ description: "Job id, \"latest\" or \"all\"" }),
    }),
    exposure: "model-only",
    executionMode: "sequential",
    async execute(_toolCallId, params) {
      const res = await hook("/hook/stop_job", { session_id: sessionId, job_id: params.job_id });
      if (res && !res.ok) {
        return { content: [{ type: "text", text: `Nothing stopped (${res.error ?? "unknown"}).` }], details: undefined };
      }
      const stopped = ((res as { result?: { stopped?: string[] } } | undefined)?.result?.stopped) ?? [];
      return { content: [{ type: "text", text: `Stopped: ${stopped.join(", ") || "nothing"}. Confirm to the user in one line.` }], details: undefined };
    },
  });

  // ---- 2. ask_user ---------------------------------------------------------------------------
  pi.registerTool({
    name: "ask_user",
    label: "Ask user",
    description:
      "Ask the user one question and stop; the answer arrives as the next message. Pass `widget` when a typed answer is better than free text: choice (one of options), multi_select (several), text (placeholder, multiline), date (min/max yyyy-mm-dd), form (<= 8 fields of type text|number|choice|date|bool).",
    promptSnippet: "ask_user: ask the user one question and wait; optional widget = choice | multi_select | text | date | form",
    parameters: Type.Object({
      question: Type.String({ description: "The question, one or two sentences" }),
      widget: Type.Optional(WIDGET_PARAM),
    }),
    exposure: "model-only",
    executionMode: "sequential",
    async execute(_toolCallId, params, _signal, _onUpdate, ctx) {
      const widget = params.widget ?? null;
      const res = await hook("/hook/ask_user", { session_id: sessionId, question: params.question, widget });
      if (res?.ok) {
        return { content: [{ type: "text", text: "Question posted. Stop here; the user's answer arrives as the next message." }], details: { question: params.question, widget }, terminate: true };
      }
      if (ctx.hasUI) {
        const answer = await ctx.ui.input("Agent question", describeWidget(params.question, widget));
        return { content: [{ type: "text", text: answer ? `User answered: ${answer}` : "User gave no answer." }], details: { question: params.question, widget, answer } };
      }
      return { content: [{ type: "text", text: "No user is reachable; proceed with your best assumption and say what you assumed." }], details: { question: params.question, widget } };
    },
  });

  // ---- 4c. peek ------------------------------------------------------------------------------
  pi.registerTool({
    name: "peek",
    label: "Peek",
    description: "Read a slice of an offloaded output by handle: lines [from,to] (1-based), chars [from,to], or grep regex; max 200 lines.",
    promptSnippet: "peek: read a slice of a big tool output by handle",
    parameters: Type.Object({
      handle: Type.String(),
      lines: Type.Optional(Type.Array(Type.Integer(), { minItems: 2, maxItems: 2 })),
      chars: Type.Optional(Type.Array(Type.Integer(), { minItems: 2, maxItems: 2 })),
      grep: Type.Optional(Type.String()),
      max: Type.Optional(Type.Integer({ minimum: 1, maximum: 200 })),
    }),
    async execute(_toolCallId, params) {
      const h = handles.get(params.handle);
      let path = h?.path;
      if (!path) {
        const remote = await hook("/hook/handle-path", { handle: params.handle });
        if (remote?.ok && typeof remote.text === "string") path = remote.text;
      }
      if (!path || !existsSync(path)) throw new Error(`unknown handle ${params.handle}`);
      const full = readFileSync(path, "utf8");
      const max = params.max ?? 200;
      let out: string;
      if (params.grep) {
        const re = new RegExp(params.grep);
        out = full
          .split("\n")
          .map((l, i) => [i + 1, l] as const)
          .filter(([, l]) => re.test(l))
          .slice(0, max)
          .map(([n, l]) => `${n}: ${l}`)
          .join("\n");
      } else if (params.chars) {
        out = full.slice(params.chars[0], params.chars[1]);
      } else {
        const [from, to] = params.lines ?? [1, max];
        out = full
          .split("\n")
          .slice(Math.max(0, from - 1), Math.min(to, from - 1 + max))
          .map((l, i) => `${from + i}: ${l}`)
          .join("\n");
      }
      if (out.length > 16_000) out = `${out.slice(0, 16_000)}\n…[truncated; narrow the range]`;
      return { content: [{ type: "text", text: out || "(empty slice)" }], details: { handle: params.handle } };
    },
  });

  // ---- 4d. remember / recall scratchpad (proposals to the write decision, never T1 writes) --
  pi.registerTool({
    name: "remember",
    label: "Remember",
    description: "Save a short named note in this chat's working set (null forgets).",
    parameters: Type.Object({ name: Type.String(), value: Type.Union([Type.String(), Type.Null()]) }),
    async execute(_toolCallId, params) {
      if (params.value === null) scratch.delete(params.name);
      else scratch.set(params.name, params.value);
      pi.appendEntry("arbos-remember", { name: params.name, value: params.value });
      void hook("/hook/remember", { session_id: sessionId, name: params.name, value: params.value });
      return { content: [{ type: "text", text: params.value === null ? `Forgot ${params.name}.` : `Remembered ${params.name}.` }], details: undefined };
    },
  });
  pi.registerTool({
    name: "recall",
    label: "Recall",
    description: "Read a note saved with remember.",
    parameters: Type.Object({ name: Type.String() }),
    async execute(_toolCallId, params) {
      const v = scratch.get(params.name);
      return { content: [{ type: "text", text: v ?? `(no note named ${params.name})` }], details: undefined };
    },
  });

  // Route guard, part 1 (prevention): every main-agent prompt carries a one-line reminder, so a resumed or
  // long session cannot drift into answering without `route` (measured 2/20 skips without it).
  pi.on("input", async (event) => {
    if (SESSION_ROLE !== "main" || event.source === "extension" || event.text.startsWith("/")) return { action: "continue" };
    if (event.text.startsWith(ROUTE_PREFIX)) return { action: "continue" };
    // A stamped steer (#1690): pi's steer inside a routed run, or the prompt that continues the
    // turn after an abort (`before_agent_start` keeps that run routed). No route reminder.
    if (event.text.startsWith(STEER_PREFIX)) return { action: "continue" };
    // D87: the per-turn context goes here, after the stable prefix, instead of into the system prompt.
    const context = STABLE_PREFIX ? await turnContextBlock() : "";
    return { action: "transform", text: `${ROUTE_PREFIX}${context}${event.text}`, images: event.images };
  });

  // Route guard, part 2 (correction): a run that still ends without `route` gets one corrective
  // continuation when pi allows one (`canContinue`: the run ended on tool calls, not on plain text).
  // When pi will not continue, the miss is logged: agentd keeps any prose as `reply`, or — if the
  // turn produced no words — rebuilds the session and asks once more (empty-answer retry).
  // Await the hook so agentd sees `route_missing` before the turn finalizes.
  pi.on("agent_before_settle", async (event) => {
    if (SESSION_ROLE !== "main" || routed) return undefined;
    if (routeCorrections >= 1 || !event.context.canContinue) {
      await hook("/hook/turn-without-route", { session_id: sessionId, corrected: false, can_continue: event.context.canContinue });
      return undefined;
    }
    routeCorrections += 1;
    correcting = true;
    await hook("/hook/turn-without-route", { session_id: sessionId, corrected: true });
    return {
      continue: true,
      entries: [
        {
          type: "custom_message",
          customType: "arbos-route-reminder",
          content:
            "Your previous turn did not start with the required `route` call. Call `route` now with the action that matches what you already did (`reply` if you answered; put nothing in `reply`). Do not repeat your answer.",
          display: false,
        },
      ],
    };
  });

  pi.on("session_compact", async () => {
    // The working set is rebuilt from `handles`/`scratch` on the next before_agent_start,
    // so it survives compaction by construction (design §6.6).
    void hook("/hook/compacted", { session_id: sessionId });
  });

  audit({ kind: "extension_loaded", rules: DENY_RULES.map((r) => r.name), code_mode: CODE_MODE, stable_prefix: STABLE_PREFIX });
}

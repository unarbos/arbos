/**
 * The transcript model: the session's visible history folded from seam
 * envelopes. One reducer is the single home for "what does an event do to the
 * screen" — the web mirror of the TUI's event switch in cmd/arbos.
 *
 * A delegated child's events fold into their own per-child transcript (the
 * sub-agent tab); the model's reasoning streams as a Thinking section.
 */

import type { ReplayEvent } from "./api";
import type {
  ContentBlock,
  Envelope,
  StopReason,
  ToolCall,
  ToolResult,
  Usage,
} from "./types";

export type TranscriptItem =
  | { kind: "user"; id: number; text: string; parts?: ContentBlock[] }
  | {
      kind: "assistant";
      id: number;
      text: string;
      streaming: boolean;
      stopReason?: StopReason;
      usage?: Usage;
    }
  | { kind: "thinking"; id: number; text: string; streaming: boolean }
  | {
      kind: "tool";
      id: number;
      call: ToolCall;
      result?: ToolResult;
      /**
       * For delegating tools: the child session running the sub-task. Set
       * live (first relayed envelope) or from the result's Details on replay;
       * it is what makes the row a clickable sub-agent tab.
       */
      childSession?: string;
    }
  | { kind: "queued"; id: number; text: string }
  | { kind: "interrupted"; id: number }
  | { kind: "error"; id: number; message: string; retryable: boolean }
  /** Outbox delivery — the agent speaking up between turns. */
  | { kind: "notice"; id: number; text: string }
  /**
   * A sub-agent that arrived without a delegate row to attach to (e.g. a
   * relay from a tool the UI doesn't know) — still a clickable tab.
   */
  | { kind: "subagent"; id: number; session: string; label: string };

export interface PendingApproval {
  requestId: string;
  call: ToolCall;
  reason?: string;
}

/** A bash command that outlived its wait and continues as a kernel job. */
export interface BackgroundJob {
  id: string;
  command: string;
}

/** A plan node armed on a clock or a callback — scheduled background work. */
export interface ScheduledTask {
  id: number;
  goal: string;
  when: string; // "every 1m" | "in 30m" | "on deps"
}

export interface ChatState {
  items: TranscriptItem[];
  turnActive: boolean;
  pendingApproval: PendingApproval | null;
  /** Live background work, shown above the composer like Cursor's terminals. */
  jobs: BackgroundJob[];
  scheduled: ScheduledTask[];
  /**
   * Live sub-agent transcripts, keyed by child session id: relayed depth>0
   * envelopes fold into a full per-child ChatState, so opening a sub-agent
   * tab renders a real chat (prose, thinking, tools), not a summary line.
   * Owned by this chat — relays only ever ride this chat's connection.
   */
  children: Record<string, ChatState>;
  nextId: number;
}

export const initialChatState: ChatState = {
  items: [],
  turnActive: false,
  pendingApproval: null,
  jobs: [],
  scheduled: [],
  children: {},
  nextId: 1,
};

export type ChatAction =
  | { type: "envelope"; env: Envelope }
  | { type: "user"; text: string; parts?: ContentBlock[] }
  | { type: "seam-error"; message: string }
  | { type: "approval-answered" }
  | { type: "replay"; items: TranscriptItem[] }
  | { type: "notice"; text: string }
  /** The user killed a background job / cancelled a scheduled node. */
  | { type: "job-removed"; id: string }
  | { type: "scheduled-removed"; id: number };

export function chatReducer(state: ChatState, action: ChatAction): ChatState {
  switch (action.type) {
    case "user":
      return push(
        { ...state, turnActive: true },
        { kind: "user", id: state.nextId, text: action.text, parts: action.parts },
      );
    case "seam-error":
      return push(state, {
        kind: "error",
        id: state.nextId,
        message: action.message,
        retryable: true,
      });
    case "approval-answered":
      return { ...state, pendingApproval: null };
    case "notice":
      return push(state, {
        kind: "notice",
        id: state.nextId,
        text: action.text,
      });
    case "job-removed":
      return { ...state, jobs: state.jobs.filter((j) => j.id !== action.id) };
    case "scheduled-removed":
      return {
        ...state,
        scheduled: state.scheduled.filter((t) => t.id !== action.id),
      };
    case "replay": {
      // Seed a resumed session's past transcript; live events append after.
      // Re-run background tracking over the replayed tool results so the
      // jobs/scheduled bar survives a resume the same way the transcript does.
      let s: ChatState = {
        ...initialChatState,
        items: action.items,
        nextId: action.items.length + 1,
      };
      for (const it of action.items) {
        if (it.kind === "tool" && it.result) {
          s = trackBackground(s, it.call, it.result);
        }
      }
      return s;
    }
    case "envelope":
      return applyEnvelope(state, action.env);
    default: {
      const never: never = action;
      throw new Error(`unhandled action ${JSON.stringify(never)}`);
    }
  }
}

function push(state: ChatState, item: TranscriptItem): ChatState {
  return { ...state, items: [...state.items, item], nextId: state.nextId + 1 };
}

function replaceAt(state: ChatState, i: number, item: TranscriptItem): ChatState {
  const items = state.items.slice();
  items[i] = item;
  return { ...state, items };
}

/** The child session a delegating tool recorded in its result's Details. */
function detailsChildSession(result: ToolResult): string | undefined {
  const d = result.Details;
  if (typeof d === "object" && d !== null && "childSession" in d) {
    const cs = (d as { childSession?: unknown }).childSession;
    if (typeof cs === "string" && cs) return cs;
  }
  return undefined;
}

function lastStreaming(state: ChatState): number {
  for (let i = state.items.length - 1; i >= 0; i--) {
    const it = state.items[i];
    if (it.kind === "assistant" && it.streaming) return i;
  }
  return -1;
}

/**
 * Route a relayed sub-agent envelope into its own per-child transcript. The
 * child's events fold through the same applyEnvelope as the parent's (depth
 * re-based to 0), so a sub-agent tab IS a chat — prose, thinking, tool cards.
 * The first envelope from an unseen child attaches the session to the oldest
 * delegating tool row still waiting for one (delegations and their relays
 * both arrive in call order); without one it becomes a standalone tab item.
 */
function applyChildEnvelope(state: ChatState, env: Envelope): ChatState {
  const sid = env.session_id;
  let s = state;

  if (!s.children[sid]) {
    const i = s.items.findIndex(
      (it) => it.kind === "tool" && !it.result && !it.childSession && it.call.Name === "delegate",
    );
    if (i >= 0) {
      const it = s.items[i] as Extract<TranscriptItem, { kind: "tool" }>;
      s = replaceAt(s, i, { ...it, childSession: sid });
    } else {
      s = push(s, {
        kind: "subagent",
        id: s.nextId,
        session: sid,
        label: "Sub-agent",
      });
    }
  }

  const updated = applyEnvelope(s.children[sid] ?? initialChatState, {
    ...env,
    depth: 0,
  });
  return { ...s, children: { ...s.children, [sid]: updated } };
}

function applyEnvelope(state: ChatState, env: Envelope): ChatState {
  // A relayed child event renders inside its own sub-agent tab, never inline:
  // the parent transcript shows the delegation row (the tab), the child's
  // transcript shows the work.
  if (env.depth > 0) return applyChildEnvelope(state, env);

  const ev = env.event;

  switch (ev.kind) {
    case "message_delta": {
      state = closeThinking(state);
      // Append only while the streaming item is still the last thing on
      // screen. Once a tool call lands after it, prose resumes as a NEW
      // segment below the tool — the transcript interleaves text and
      // activity in event order, the way Cursor renders a turn.
      const i = lastStreaming(state);
      if (i === state.items.length - 1 && i >= 0) {
        const it = state.items[i] as Extract<TranscriptItem, { kind: "assistant" }>;
        return replaceAt(state, i, { ...it, text: it.text + ev.data.text });
      }
      return push({ ...finalize(state), turnActive: true }, {
        kind: "assistant",
        id: state.nextId,
        text: ev.data.text,
        streaming: true,
      });
    }

    case "reasoning_delta": {
      // Reasoning is a transcript section — Cursor's collapsible "Thinking"
      // block, streamed in place above the answer it produces.
      const last = state.items[state.items.length - 1];
      if (last && last.kind === "thinking" && last.streaming) {
        return replaceAt(state, state.items.length - 1, {
          ...last,
          text: last.text + ev.data.text,
        });
      }
      return push({ ...finalize(state), turnActive: true }, {
        kind: "thinking",
        id: state.nextId,
        text: ev.data.text,
        streaming: true,
      });
    }

    case "tool_started":
      return push(closeThinking(state), {
        kind: "tool",
        id: state.nextId,
        call: ev.data.call,
      });

    case "tool_finished": {
      for (let i = state.items.length - 1; i >= 0; i--) {
        const it = state.items[i];
        if (it.kind === "tool" && !it.result && it.call.ID === ev.data.result.CallID) {
          // A delegating tool's result names its true child session. The live
          // guess paired relays to rows by arrival order, which parallel
          // children can scramble — the result is authoritative, so re-pair.
          const s = replaceAt(state, i, {
            ...it,
            result: ev.data.result,
            childSession: detailsChildSession(ev.data.result) ?? it.childSession,
          });
          return trackBackground(s, it.call, ev.data.result);
        }
      }
      return state;
    }

    case "turn_complete": {
      let s: ChatState = { ...closeThinking(state), turnActive: false };
      const i = lastStreaming(s);
      if (i >= 0) {
        // The transcript may hold several prose segments from this turn
        // (interleaved with tool calls), so keep the accumulated deltas —
        // final_response is the whole turn's text and would duplicate them.
        const it = s.items[i] as Extract<TranscriptItem, { kind: "assistant" }>;
        s = replaceAt(s, i, {
          ...it,
          streaming: false,
          stopReason: ev.data.stop_reason,
          usage: ev.data.usage,
        });
      } else if (ev.data.final_response) {
        s = push(s, {
          kind: "assistant",
          id: s.nextId,
          text: ev.data.final_response,
          streaming: false,
          stopReason: ev.data.stop_reason,
          usage: ev.data.usage,
        });
      }
      return s;
    }

    case "interrupted": {
      const s = finalize(closeThinking(state));
      return {
        ...push(s, { kind: "interrupted", id: s.nextId }),
        turnActive: false,
        pendingApproval: null,
      };
    }

    case "error": {
      const s = finalize(closeThinking(state));
      return {
        ...push(s, {
          kind: "error",
          id: s.nextId,
          message: `${ev.data.category}: ${ev.data.error}`,
          retryable: ev.data.retryable,
        }),
        turnActive: false,
        pendingApproval: null,
      };
    }

    case "queued":
      return push(state, { kind: "queued", id: state.nextId, text: ev.data.text });

    case "approval_request":
      // A child's request bubbles up the relay (ADR-0018); the response
      // routes back down by request_id, so depth doesn't matter here.
      return {
        ...state,
        pendingApproval: {
          requestId: ev.data.request_id,
          call: ev.data.call,
          reason: ev.data.reason,
        },
      };

    default: {
      const never: never = ev;
      void never;
      return state;
    }
  }
}

function finalize(state: ChatState): ChatState {
  const i = lastStreaming(state);
  if (i < 0) return state;
  const it = state.items[i] as Extract<TranscriptItem, { kind: "assistant" }>;
  return replaceAt(state, i, { ...it, streaming: false });
}

/** Settle the streaming Thinking section, if any, once other output arrives. */
function closeThinking(state: ChatState): ChatState {
  for (let i = state.items.length - 1; i >= 0; i--) {
    const it = state.items[i];
    if (it.kind === "thinking" && it.streaming) {
      return replaceAt(state, i, { ...it, streaming: false });
    }
  }
  return state;
}

/* ------------------------------------------------------------------ */
/* Background tracking: fold tool results into the live jobs/scheduled */
/* lists. Best-effort and session-local — derived from the same text   */
/* the model reads, so the bar never disagrees with the transcript.    */
/* ------------------------------------------------------------------ */

const JOB_BACKGROUNDED = /in the background as job ([\w.-]+) \(pid \d+\)/;
const JOB_FINISHED = /background job ([\w.-]+) finished:|Job ([\w.-]+) (?:exited with code|killed)/g;
const TERMINAL_STATUS = new Set(["done", "failed", "cancelled"]);

function objArgs(call: ToolCall): Record<string, unknown> {
  return typeof call.Args === "object" && call.Args !== null
    ? (call.Args as Record<string, unknown>)
    : {};
}

function whenLabel(when: Record<string, unknown> | undefined): string {
  if (!when) return "";
  if (typeof when.every === "string" && when.every) return `every ${when.every}`;
  if (typeof when.after === "string" && when.after) return `in ${when.after}`;
  if (when.onDeps) return "on deps";
  return "";
}

function trackBackground(
  state: ChatState,
  call: ToolCall,
  result: ToolResult,
): ChatState {
  let { jobs, scheduled } = state;

  // Any result can carry a completion notice for an earlier job.
  for (const m of result.Content.matchAll(JOB_FINISHED)) {
    const id = m[1] ?? m[2];
    jobs = jobs.filter((j) => j.id !== id);
  }

  if (!result.IsError && call.Name === "bash") {
    const m = result.Content.match(JOB_BACKGROUNDED);
    if (m) {
      const a = objArgs(call);
      const command = typeof a.command === "string" ? a.command : "";
      jobs = [...jobs.filter((j) => j.id !== m[1]), { id: m[1], command }];
    }
  }

  if (!result.IsError && call.Name === "plan") {
    const a = objArgs(call);
    if (a.op === "add" && Array.isArray(a.nodes)) {
      // The ack's first line ("Added #5, #6.") assigns ids in node order.
      const firstLine = result.Content.split("\n", 1)[0];
      const ids = [...firstLine.matchAll(/#(\d+)/g)].map((m) => Number(m[1]));
      if (ids.length === a.nodes.length) {
        const added: ScheduledTask[] = [];
        for (let i = 0; i < ids.length; i++) {
          const node = a.nodes[i] as Record<string, unknown>;
          const when = whenLabel(node.when as Record<string, unknown> | undefined);
          if (!when) continue; // un-triggered tasks aren't background work
          const goal = typeof node.goal === "string" ? node.goal : "";
          added.push({ id: ids[i], goal, when });
        }
        if (added.length > 0) {
          const ours = new Set(added.map((t) => t.id));
          scheduled = [...scheduled.filter((t) => !ours.has(t.id)), ...added];
        }
      }
    }
    if (a.op === "update" && TERMINAL_STATUS.has(String(a.status))) {
      scheduled = scheduled.filter((t) => t.id !== Number(a.node));
    }
  }

  if (jobs === state.jobs && scheduled === state.scheduled) return state;
  return { ...state, jobs, scheduled };
}

/**
 * Fold a replayed event log into transcript items — the persisted-history
 * mirror of applyEnvelope. Tool calls come from assistant messages; results
 * attach to them by call id.
 */
export function replayToItems(events: ReplayEvent[]): TranscriptItem[] {
  const items: TranscriptItem[] = [];
  const toolIndex = new Map<string, number>();
  let id = 1;

  for (const ev of events) {
    switch (ev.type) {
      case "user":
        items.push({ kind: "user", id: id++, text: ev.text, parts: ev.parts });
        break;
      case "assistant": {
        if (ev.text) {
          items.push({ kind: "assistant", id: id++, text: ev.text, streaming: false });
        }
        for (const call of ev.tool_calls ?? []) {
          toolIndex.set(call.ID, items.length);
          items.push({ kind: "tool", id: id++, call });
        }
        break;
      }
      case "tool_result": {
        const i = toolIndex.get(ev.call_id);
        const it = i != null ? items[i] : undefined;
        if (it && it.kind === "tool") {
          const result: ToolResult = {
            CallID: ev.call_id,
            Content: ev.content ?? "",
            IsError: ev.is_error ?? false,
            Details: ev.details,
          };
          // A delegating tool recorded its child session in Details — restore
          // the link so the row stays an openable sub-agent tab after resume.
          items[i!] = {
            ...it,
            result,
            childSession: detailsChildSession(result),
          };
        }
        break;
      }
      case "interrupted":
        items.push({ kind: "interrupted", id: id++ });
        break;
      default: {
        const never: never = ev;
        void never;
      }
    }
  }
  return items;
}

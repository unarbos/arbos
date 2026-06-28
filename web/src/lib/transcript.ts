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
  Citation,
  ContentBlock,
  Envelope,
  ErrorKind,
  Question,
  StopReason,
  ToolCall,
  ToolResult,
  Usage,
} from "./types";

/**
 * What an error row attributes the failure to: the kernel's ErrorKind, plus
 * "connection" for a seam/transport failure that never reached the kernel. The
 * error card maps it to a title, icon, and tone.
 */
export type ErrorCategory = ErrorKind | "connection";

export type TranscriptItem =
  /**
   * `seq` is the event's position in the persisted session log, present only
   * on items seeded from a replay. Rewind-and-edit forks at it directly;
   * optimistic live items carry none and fall back to text-verified matching.
   */
  | { kind: "user"; id: number; text: string; parts?: ContentBlock[]; seq?: number; author?: string }
  | {
      kind: "assistant";
      id: number;
      text: string;
      streaming: boolean;
      /** The event's position in the persisted log, present only on replay-
       *  seeded items. The discussion-branching highlight anchors to it. */
      seq?: number;
      stopReason?: StopReason;
      usage?: Usage;
      /** Web-search sources the provider grounded this message on (a "Sources"
       *  strip). Set by the citations event live, or from replay. */
      citations?: Citation[];
      /** Provider-generated images (image_generation server tool), rendered
       *  inline under the prose. Set by the images event live, or from
       *  replay's assistant parts. */
      images?: ContentBlock[];
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
      /**
       * Arguments still streaming in: the bytes-so-far plus the raw argument
       * JSON accumulated so far, for the live "composing" card the row shows
       * before the finished call lands. Set by tool_progress (which may arrive
       * before the call is whole, so `call.Args` is absent), cleared by
       * tool_started once the real call is in hand. The partial `args` is what
       * lets a write/edit stream its file body in live. Live-only — a replayed
       * transcript builds the row straight from the finished call.
       */
      composing?: { bytes: number; args: string };
      /**
       * A running tool's presentation Details, streamed by a tool_details event
       * before the result lands — chiefly a bash command's journaled job id, so
       * the terminal card can open a live tail in a side panel while the command
       * is still running. Live-only; once the result arrives its Details is
       * authoritative (and a resumed transcript reads the job id from there).
       */
      liveDetails?: unknown;
      /**
       * The turn ended (stop or error) before this call's result arrived: the
       * row renders as "stopped" instead of spinning forever — no result is
       * ever coming for it.
       */
      interrupted?: boolean;
    }
  | {
      kind: "queued";
      id: number;
      text: string;
      /** Door the prompt arrived through when not this frontend (e.g.
       *  "telegram") — render it as the user speaking, not a queue ack. */
      origin?: string;
      /** Display name of the multi-party guest who sent it, when known. */
      author?: string;
      /** The prompt's non-text content (a photo sent from the phone). */
      parts?: ContentBlock[];
    }
  | { kind: "interrupted"; id: number }
  | {
      kind: "error";
      id: number;
      /** The raw error detail (the kernel's message or a seam failure). */
      message: string;
      retryable: boolean;
      /**
       * What the error is attributed to — the kernel ErrorKind or "connection"
       * for a seam failure — which the error card maps to a title, icon, and
       * explanation.
       */
      category: ErrorCategory;
    }
  /** Outbox delivery — the agent speaking up between turns. */
  | { kind: "notice"; id: number; text: string }
  /**
   * A human-to-human side note (chat_note), folded inline into the conversation
   * timeline (ADR-0038/0041): people on the session talking to EACH OTHER, never
   * to the model. Rendered as a distinct, dim side-chat row — in the timeline,
   * not a separate panel.
   */
  | { kind: "chatnote"; id: number; text: string; author?: string }
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

/** The ask tool's pending form: questions awaiting the user's answers. */
export interface PendingQuestions {
  requestId: string;
  title?: string;
  questions: Question[];
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
  pendingQuestions: PendingQuestions | null;
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
  pendingQuestions: null,
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
  | { type: "questions-answered" }
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
        category: "connection",
      });
    case "approval-answered":
      return { ...state, pendingApproval: null };
    case "questions-answered":
      return { ...state, pendingQuestions: null };
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

/** Index of the latest still-open tool row (no result yet) for a call id, or
 *  -1 if none. Every tool event — progress, details, started, finished —
 *  attaches to its row by this same scan. */
function findOpenTool(items: TranscriptItem[], callID: string): number {
  for (let i = items.length - 1; i >= 0; i--) {
    const it = items[i];
    if (it.kind === "tool" && it.call.ID === callID && !it.result) return i;
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

    case "citations": {
      // Sources for the message just streamed: they ride the provider's final
      // chunk, so they land after the content deltas (the assistant item is
      // still the latest streaming one) and before turn_complete. Attach to
      // the most recent assistant segment; if none has prose (a search that
      // grounded a tool-only turn), there is nothing to annotate.
      for (let i = state.items.length - 1; i >= 0; i--) {
        const it = state.items[i];
        if (it.kind === "assistant") {
          return replaceAt(state, i, { ...it, citations: ev.data.citations });
        }
      }
      return state;
    }

    case "images": {
      // Generated images for the message just streamed: like citations they
      // ride the provider's final chunk, so the assistant segment they belong
      // to is the most recent one. An image-only turn (no prose) gets a fresh
      // assistant item so the images still render.
      for (let i = state.items.length - 1; i >= 0; i--) {
        const it = state.items[i];
        if (it.kind === "assistant") {
          return replaceAt(state, i, { ...it, images: ev.data.images });
        }
      }
      return push({ ...finalize(state), turnActive: true }, {
        kind: "assistant",
        id: state.nextId,
        text: "",
        streaming: false,
        images: ev.data.images,
      });
    }

    case "tool_progress": {
      // A tool call's arguments streaming in (e.g. a canvas's HTML body): show
      // a live composing row in the gap before the finished call lands. The
      // first sighting of a call id seeds the row — finalizing any open prose,
      // since the model has moved on from text to composing the call — and
      // later updates just grow its byte count.
      const i = findOpenTool(state.items, ev.data.call_id);
      if (i >= 0) {
        const it = state.items[i] as Extract<TranscriptItem, { kind: "tool" }>;
        const args = (it.composing?.args ?? "") + (ev.data.args_delta ?? "");
        return replaceAt(state, i, { ...it, composing: { bytes: ev.data.bytes, args } });
      }
      return push({ ...finalize(closeThinking(state)), turnActive: true }, {
        kind: "tool",
        id: state.nextId,
        call: { ID: ev.data.call_id, Name: ev.data.name },
        composing: { bytes: ev.data.bytes, args: ev.data.args_delta ?? "" },
      });
    }

    case "tool_details": {
      // A running tool handed up a presentation fact (a bash job id) before
      // its result: attach it to the matching open row so the terminal card
      // can offer a live tail while the command is still running. Only ever
      // updates a result-less row — once the result lands, its Details wins.
      const i = findOpenTool(state.items, ev.data.call_id);
      if (i < 0) return state;
      const it = state.items[i] as Extract<TranscriptItem, { kind: "tool" }>;
      return replaceAt(state, i, { ...it, liveDetails: ev.data.details });
    }

    case "tool_started": {
      // The finished call: adopt the composing row it grew from (so the card
      // transitions in place, no flicker), else start a fresh row.
      const call = ev.data.call;
      const s = closeThinking(state);
      const i = findOpenTool(s.items, call.ID);
      if (i >= 0) {
        const it = s.items[i] as Extract<TranscriptItem, { kind: "tool" }>;
        return replaceAt(s, i, { ...it, call, composing: undefined });
      }
      return push(s, { kind: "tool", id: s.nextId, call });
    }

    case "tool_finished": {
      const i = findOpenTool(state.items, ev.data.result.CallID);
      if (i < 0) return state;
      const it = state.items[i] as Extract<TranscriptItem, { kind: "tool" }>;
      // A delegating tool's result names its true child session. The live
      // guess paired relays to rows by arrival order, which parallel
      // children can scramble — the result is authoritative, so re-pair.
      let s = replaceAt(state, i, {
        ...it,
        result: ev.data.result,
        childSession: detailsChildSession(ev.data.result) ?? it.childSession,
      });
      // An ask answered elsewhere (another frontend, the TUI) resolves here
      // too — its panel must not linger once the result lands.
      if (it.call.Name === "ask" && s.pendingQuestions) {
        s = { ...s, pendingQuestions: null };
      }
      return trackBackground(s, it.call, ev.data.result);
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
      const s = settleTools(finalize(closeThinking(state)));
      return {
        ...push(s, { kind: "interrupted", id: s.nextId }),
        turnActive: false,
        pendingApproval: null,
        pendingQuestions: null,
      };
    }

    case "error": {
      const s = settleTools(finalize(closeThinking(state)));
      return {
        ...push(s, {
          kind: "error",
          id: s.nextId,
          message: ev.data.error,
          retryable: ev.data.retryable,
          category: ev.data.category,
        }),
        turnActive: false,
        pendingApproval: null,
        pendingQuestions: null,
      };
    }

    case "queued":
      return push(state, {
        kind: "queued",
        id: state.nextId,
        text: ev.data.text,
        origin: ev.data.origin,
        author: ev.data.author,
        parts: ev.data.parts,
      });

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

    case "question_request":
      // The ask tool paused the turn for the user's structured answers — the
      // panel renders above the composer until they Continue or Skip.
      return {
        ...state,
        pendingQuestions: {
          requestId: ev.data.request_id,
          title: ev.data.title,
          questions: ev.data.questions,
        },
      };

    case "chat_note":
      // Human-to-human side chat, folded inline into the timeline (ADR-0038):
      // a distinct side-note row, excluded from the model projection but shown
      // in the conversation where it belongs — not a separate panel.
      return push(state, {
        kind: "chatnote",
        id: state.nextId,
        text: ev.data.text,
        author: ev.data.author,
      });

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

/**
 * Settle tool rows still awaiting results once the turn dies (stop or error):
 * the kernel will never deliver them, so a live spinner — a composing card
 * mid-argument-stream, a running terminal card — would spin forever.
 */
function settleTools(state: ChatState): ChatState {
  let items: TranscriptItem[] | null = null;
  for (let i = 0; i < state.items.length; i++) {
    const it = state.items[i];
    if (it.kind === "tool" && !it.result && !it.interrupted) {
      items ??= state.items.slice();
      items[i] = { ...it, interrupted: true, composing: undefined };
    }
  }
  return items ? { ...state, items } : state;
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
 * attach to them by call id. (Side notes fold inline as `chatnote` items,
 * handled in replayToItems — there is no separate People-panel list anymore.)
 */

/** A discussion-branch anchor reduced to its latest status, for the parent
 *  transcript's "discussed" markers. The branch_anchor replay rows are
 *  append-only (open -> accepted/discarded), so the LAST row per branch wins. */
export interface BranchAnchor {
  child: string;
  seq: number;
  quote: string;
  /** Full text of the containing message (the branch's scope). */
  message?: string;
  status: "open" | "accepted" | "discarded";
  summary?: string;
}

/** Collapse a session replay's branch_anchor rows into the current state of each
 *  branch (latest row per child wins), so the transcript can mark a branched
 *  message and offer to reopen its child discussion. */
export function anchorsFromReplay(events: ReplayEvent[]): BranchAnchor[] {
  const byChild = new Map<string, BranchAnchor>();
  for (const ev of events) {
    if (ev.type !== "branch_anchor") continue;
    byChild.set(ev.branch, {
      child: ev.branch,
      seq: ev.anchor_seq,
      quote: ev.quote,
      message: ev.anchor_message,
      status: ev.branch_status,
      summary: ev.summary,
    });
  }
  return [...byChild.values()];
}

export function replayToItems(events: ReplayEvent[]): TranscriptItem[] {
  const items: TranscriptItem[] = [];
  const toolIndex = new Map<string, number>();
  let id = 1;

  for (const ev of events) {
    switch (ev.type) {
      case "user":
        items.push({ kind: "user", id: id++, text: ev.text, parts: ev.parts, seq: ev.seq, author: ev.author });
        break;
      case "assistant": {
        const images = (ev.parts ?? []).filter((p) => p.type === "image");
        if (ev.text || images.length > 0) {
          items.push({
            kind: "assistant",
            id: id++,
            text: ev.text ?? "",
            streaming: false,
            seq: ev.seq,
            citations: ev.citations,
            images: images.length > 0 ? images : undefined,
          });
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
        // Calls cut off by the stop never got results; settle them so the
        // replayed rows render "stopped" instead of spinning forever.
        for (let i = 0; i < items.length; i++) {
          const it = items[i];
          if (it.kind === "tool" && !it.result) {
            items[i] = { ...it, interrupted: true };
          }
        }
        items.push({ kind: "interrupted", id: id++ });
        break;
      case "chat_note":
        // Human-to-human side chat, folded inline into the timeline (ADR-0038):
        // a side-note row in the conversation, never part of the model
        // projection.
        items.push({ kind: "chatnote", id: id++, text: ev.text, author: ev.author });
        break;
      case "branch_anchor":
        // Discussion-branch bookkeeping: surfaced via anchorsFromReplay as a
        // marker on the branched message, never as a transcript row of its own.
        break;
      default: {
        const never: never = ev;
        void never;
      }
    }
  }
  return items;
}

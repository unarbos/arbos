/**
 * The transcript model: the session's visible history folded from seam
 * envelopes. One reducer is the single home for "what does an event do to the
 * screen" — the web mirror of the TUI's event switch in cmd/arbos.
 *
 * Like the TUI: a delegated child's prose and the model's reasoning are
 * ephemeral previews while the turn runs, never part of the permanent
 * transcript; the final answer replaces the streamed window on turn end.
 */

import type {
  Envelope,
  StopReason,
  ToolCall,
  ToolResult,
  Usage,
} from "./types";

export type TranscriptItem =
  | { kind: "user"; id: number; text: string }
  | {
      kind: "assistant";
      id: number;
      text: string;
      streaming: boolean;
      stopReason?: StopReason;
      usage?: Usage;
    }
  | {
      kind: "tool";
      id: number;
      call: ToolCall;
      result?: ToolResult;
      depth: number;
      startedAt: number;
      completedAt?: number;
    }
  | { kind: "queued"; id: number; text: string }
  | { kind: "interrupted"; id: number }
  | { kind: "error"; id: number; message: string; retryable: boolean };

export interface PendingApproval {
  requestId: string;
  call: ToolCall;
  reason?: string;
}

export interface ChatState {
  items: TranscriptItem[];
  turnActive: boolean;
  /** One-line live preview of reasoning or child narration; never persisted. */
  ephemeral: string;
  pendingApproval: PendingApproval | null;
  nextId: number;
}

export const initialChatState: ChatState = {
  items: [],
  turnActive: false,
  ephemeral: "",
  pendingApproval: null,
  nextId: 1,
};

export type ChatAction =
  | { type: "envelope"; env: Envelope }
  | { type: "user"; text: string }
  | { type: "seam-error"; message: string }
  | { type: "approval-answered" };

const EPHEMERAL_MAX = 200;

export function chatReducer(state: ChatState, action: ChatAction): ChatState {
  switch (action.type) {
    case "user":
      return push(
        { ...state, turnActive: true },
        { kind: "user", id: state.nextId, text: action.text },
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

function lastStreaming(state: ChatState): number {
  for (let i = state.items.length - 1; i >= 0; i--) {
    const it = state.items[i];
    if (it.kind === "assistant" && it.streaming) return i;
  }
  return -1;
}

function ephemeral(state: ChatState, text: string): ChatState {
  const line = (state.ephemeral + text).replace(/\s+/g, " ");
  return { ...state, ephemeral: line.slice(-EPHEMERAL_MAX), turnActive: true };
}

function applyEnvelope(state: ChatState, env: Envelope): ChatState {
  const ev = env.event;
  const child = env.depth > 0;

  switch (ev.kind) {
    case "message_delta": {
      // A child's prose is internal narration, not this turn's answer — it
      // only feeds the one-line ephemeral preview (the TUI's rule).
      if (child) return ephemeral(state, ev.data.text);
      const i = lastStreaming(state);
      if (i >= 0) {
        const it = state.items[i] as Extract<TranscriptItem, { kind: "assistant" }>;
        return replaceAt(state, i, { ...it, text: it.text + ev.data.text });
      }
      return push({ ...state, turnActive: true, ephemeral: "" }, {
        kind: "assistant",
        id: state.nextId,
        text: ev.data.text,
        streaming: true,
      });
    }

    case "reasoning_delta":
      return ephemeral(state, ev.data.text);

    case "tool_started":
      return push({ ...state, ephemeral: "" }, {
        kind: "tool",
        id: state.nextId,
        call: ev.data.call,
        depth: env.depth,
        startedAt: Date.now(),
      });

    case "tool_finished": {
      for (let i = state.items.length - 1; i >= 0; i--) {
        const it = state.items[i];
        if (it.kind === "tool" && !it.result && it.call.ID === ev.data.result.CallID) {
          return replaceAt(state, i, {
            ...it,
            result: ev.data.result,
            completedAt: Date.now(),
          });
        }
      }
      return state;
    }

    case "turn_complete": {
      if (child) return state;
      let s: ChatState = { ...state, turnActive: false, ephemeral: "" };
      const i = lastStreaming(s);
      if (i >= 0) {
        const it = s.items[i] as Extract<TranscriptItem, { kind: "assistant" }>;
        // The final response is the canonical full text; prefer it over the
        // accumulated deltas (clean reflow, no lost chunk).
        s = replaceAt(s, i, {
          ...it,
          text: ev.data.final_response || it.text,
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
      if (child) return state;
      const s = finalize(state);
      return {
        ...push(s, { kind: "interrupted", id: s.nextId }),
        turnActive: false,
        ephemeral: "",
        pendingApproval: null,
      };
    }

    case "error": {
      if (child) return state;
      const s = finalize(state);
      return {
        ...push(s, {
          kind: "error",
          id: s.nextId,
          message: `${ev.data.category}: ${ev.data.error}`,
          retryable: ev.data.retryable,
        }),
        turnActive: false,
        ephemeral: "",
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

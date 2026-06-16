/**
 * The seam contract: TypeScript mirrors of the Go wire vocabulary.
 *
 *  - control frames         — internal/control/server.go
 *  - interaction vocabulary — internal/core/{intent,kernelevent,envelope}.go
 *                             via the {kind,data} codec in interaction.go
 *
 * Field-name casing is load-bearing: types with Go json tags are snake_case;
 * types without (ToolCall, ToolResult, Usage) keep Go's exported names.
 */

export interface ToolCall {
  ID: string;
  Name: string;
  Args?: unknown;
}

/** Inline base64 image (mirrors core.ImageData). */
export interface ImageData {
  data: string;
  mimeType: string;
}

/** Inline base64 document, e.g. a PDF (mirrors core.FileData). */
export interface FileData {
  data: string;
  mimeType: string;
  name?: string;
}

/**
 * One piece of multimodal content (mirrors core.ContentBlock). A prompt's
 * attached images ride along as image blocks so a vision model sees them, and
 * documents (PDFs) as file blocks the provider parses natively.
 */
export type ContentBlock =
  | { type: "text"; text: string }
  | { type: "image"; image: ImageData }
  | { type: "file"; file: FileData };

export interface ToolResult {
  CallID: string;
  Content: string;
  IsError: boolean;
  /** Structured per-tool data the model never sees (e.g. edit's diff). */
  Details?: unknown;
}

/**
 * One web-search source the provider grounded an assistant message on (mirrors
 * core.Citation). It has Go json tags, so its fields are snake_case. Unlike a
 * tool call it is not dispatched — the provider runs the search server-side and
 * reports the sources it used.
 */
export interface Citation {
  url: string;
  title?: string;
  content?: string;
  start_index?: number;
  end_index?: number;
}

export interface Usage {
  PromptTokens: number;
  CompletionTokens: number;
  TotalTokens: number;
}

export type StopReason = "answered" | "max_steps" | "terminated" | "length_limit";

/**
 * Kernel ErrorEvent category (core.ErrorKind): what kind of failure ended the
 * turn, so the error card can render an apt title and tone instead of parsing
 * the message string.
 */
export type ErrorKind = "provider" | "history" | "persist" | "internal";

/** One selectable answer to a Question (mirrors core.QuestionOption). */
export interface QuestionOption {
  id: string;
  label: string;
}

/** One multiple-choice question put to the user (mirrors core.Question). */
export interface Question {
  id: string;
  prompt: string;
  options: QuestionOption[];
  allow_multiple?: boolean;
}

/** The user's answer to one Question (mirrors core.QuestionAnswer). */
export interface QuestionAnswer {
  question_id: string;
  selected_ids?: string[];
  other_text?: string;
}

export type KernelEvent =
  | { kind: "message_delta"; data: { text: string } }
  | { kind: "reasoning_delta"; data: { text: string } }
  | { kind: "citations"; data: { citations: Citation[] } }
  | { kind: "images"; data: { images: ContentBlock[] } }
  | {
      kind: "tool_progress";
      data: { call_id: string; name: string; bytes: number; args_delta?: string };
    }
  | { kind: "tool_details"; data: { call_id: string; details?: unknown } }
  | { kind: "tool_started"; data: { call: ToolCall } }
  | { kind: "tool_finished"; data: { result: ToolResult } }
  | {
      kind: "turn_complete";
      data: { final_response: string; stop_reason: StopReason; usage: Usage };
    }
  | { kind: "interrupted"; data?: Record<string, never> }
  | {
      kind: "error";
      data: { category: ErrorKind; retryable: boolean; error: string };
    }
  | { kind: "queued"; data: { text: string; origin?: string; author?: string; parts?: ContentBlock[] } }
  | {
      kind: "approval_request";
      data: { request_id: string; call: ToolCall; reason?: string };
    }
  | {
      kind: "question_request";
      data: { request_id: string; title?: string; questions: Question[] };
    }
  | {
      kind: "chat_note";
      data: { text: string; author?: string; origin?: string; ts: number };
    };

export interface Envelope {
  session_id: string;
  depth: number;
  event: KernelEvent;
}

export type Intent =
  | { kind: "prompt"; data: { text: string; parts?: ContentBlock[]; author?: string } }
  | { kind: "steer"; data: { text: string; parts?: ContentBlock[]; author?: string } }
  | { kind: "interrupt"; data: Record<string, never> }
  | {
      kind: "approval_response";
      data: { request_id: string; approved: boolean; reason?: string };
    }
  | {
      kind: "question_response";
      data: {
        request_id: string;
        answers?: QuestionAnswer[];
        details?: string;
        skipped?: boolean;
      };
    }
  | { kind: "chat_note"; data: { text: string; author?: string } };

export type ClientFrame =
  | { type: "open"; session_id?: string }
  | { type: "intent"; intent: Intent }
  | { type: "set_model"; model: string }
  | { type: "set_web_search"; enabled: boolean }
  | { type: "set_web_fetch"; enabled: boolean }
  | { type: "set_image_gen"; enabled: boolean }
  /**
   * Branch the bound session at through_seq (last source event to keep;
   * negative = empty branch, omitted = whole log) and rebind to the branch.
   */
  | {
      type: "fork";
      session_id?: string;
      new_session_id?: string;
      through_seq?: number;
    };

export type ServerFrame =
  | { type: "opened"; session_id: string }
  | { type: "switched"; session_id: string }
  | { type: "forked"; session_id: string }
  | { type: "event"; envelope: Envelope }
  | { type: "error"; error: string }
  // Gateway-level outbox delivery: the agent's voice between turns
  // (scheduled firings, finished background work). Not part of control.Serve.
  /** session names the owning chat; "" / absent = ambient broadcast. */
  | { type: "notice"; text: string; session?: string; created_at: number };

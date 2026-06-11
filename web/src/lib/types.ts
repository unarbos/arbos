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

/**
 * One piece of multimodal content (mirrors core.ContentBlock). A prompt's
 * attached images ride along as image blocks so a vision model sees them.
 */
export type ContentBlock =
  | { type: "text"; text: string }
  | { type: "image"; image: ImageData };

export interface ToolResult {
  CallID: string;
  Content: string;
  IsError: boolean;
  /** Structured per-tool data the model never sees (e.g. edit's diff). */
  Details?: unknown;
}

export interface Usage {
  PromptTokens: number;
  CompletionTokens: number;
  TotalTokens: number;
}

export type StopReason = "answered" | "max_steps" | "terminated" | "length_limit";

export type KernelEvent =
  | { kind: "message_delta"; data: { text: string } }
  | { kind: "reasoning_delta"; data: { text: string } }
  | { kind: "tool_started"; data: { call: ToolCall } }
  | { kind: "tool_finished"; data: { result: ToolResult } }
  | {
      kind: "turn_complete";
      data: { final_response: string; stop_reason: StopReason; usage: Usage };
    }
  | { kind: "interrupted"; data?: Record<string, never> }
  | { kind: "error"; data: { category: string; retryable: boolean; error: string } }
  | { kind: "queued"; data: { text: string } }
  | {
      kind: "approval_request";
      data: { request_id: string; call: ToolCall; reason?: string };
    };

export interface Envelope {
  session_id: string;
  depth: number;
  event: KernelEvent;
}

export type Intent =
  | { kind: "prompt"; data: { text: string; parts?: ContentBlock[] } }
  | { kind: "steer"; data: { text: string; parts?: ContentBlock[] } }
  | { kind: "interrupt"; data: Record<string, never> }
  | {
      kind: "approval_response";
      data: { request_id: string; approved: boolean; reason?: string };
    };

export type ClientFrame =
  | { type: "open"; session_id?: string }
  | { type: "intent"; intent: Intent }
  | { type: "set_model"; model: string };

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

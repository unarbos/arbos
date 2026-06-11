/** REST surface of the gateway: session history for the picker and replay. */

import type { ContentBlock, ToolCall } from "./types";

export interface SessionSummary {
  id: string;
  title: string;
  updated_at: number; // unix milliseconds
}

export type ReplayEvent =
  | { type: "user"; text: string; parts?: ContentBlock[] }
  | { type: "assistant"; text?: string; tool_calls?: ToolCall[] }
  | {
      type: "tool_result";
      call_id: string;
      content?: string;
      is_error?: boolean;
      details?: unknown;
    }
  | { type: "interrupted" };

/** One selectable model from the provider's catalog (OpenRouter's listing). */
export interface ModelOption {
  id: string;
  name?: string;
  context_length?: number;
}

/** The composer's model picker: the provider catalog + the active selection. */
export interface ModelCatalog {
  models: ModelOption[];
  current: string;
}

export async function fetchModels(): Promise<ModelCatalog> {
  const res = await fetch("/api/models");
  if (!res.ok) throw new Error(`models: ${res.status}`);
  const body = (await res.json()) as {
    models?: ModelOption[];
    current?: string;
  };
  return { models: body.models ?? [], current: body.current ?? "" };
}

export async function fetchSessions(): Promise<SessionSummary[]> {
  const res = await fetch("/api/sessions");
  if (!res.ok) throw new Error(`sessions: ${res.status}`);
  const body = (await res.json()) as { sessions: SessionSummary[] };
  return body.sessions;
}

export async function fetchReplay(sessionId: string): Promise<ReplayEvent[]> {
  const res = await fetch(`/api/sessions/${encodeURIComponent(sessionId)}/events`);
  if (!res.ok) throw new Error(`events: ${res.status}`);
  const body = (await res.json()) as { events: ReplayEvent[] };
  return body.events;
}

/** One scheduler-spawned run owned by a chat (a plan node's agent firing). */
export interface ChildRun {
  id: string;
  node?: number;
  active: boolean;
  created_at: number; // unix milliseconds
  updated_at: number; // unix milliseconds
}

export async function fetchChildren(sessionId: string): Promise<ChildRun[]> {
  const res = await fetch(
    `/api/sessions/${encodeURIComponent(sessionId)}/children`,
  );
  if (!res.ok) throw new Error(`children: ${res.status}`);
  const body = (await res.json()) as { children: ChildRun[] };
  return body.children;
}

/** One armed plan node — a standing obligation, wherever it was created. */
export interface StandingTask {
  node: number;
  goal: string;
  when?: string;
  chat?: string;
  status: string;
  outcome?: string;
}

/** One recent machine-spawned session, across all chats. */
export interface ActivityRun {
  id: string;
  chat: string;
  node?: number;
  kind: "scheduled" | "delegate";
  active: boolean;
  updated_at: number; // unix milliseconds
  /** The owning plan node is no longer standing (cancelled/finished) — history, not a live recurrence. */
  stale?: boolean;
}

export interface Activity {
  standing: StandingTask[];
  runs: ActivityRun[];
}

/** The whole-organism view: every standing obligation + recent autonomous runs. */
export async function fetchActivity(): Promise<Activity> {
  const res = await fetch("/api/activity");
  if (!res.ok) throw new Error(`activity: ${res.status}`);
  return (await res.json()) as Activity;
}

/** SIGKILL a background job (the ✕ on a background terminal). */
export async function killJob(id: string): Promise<void> {
  const res = await fetch(`/api/jobs/${encodeURIComponent(id)}/kill`, {
    method: "POST",
  });
  if (!res.ok) throw new Error(`kill job: ${res.status}`);
}

/** Cancel a plan node (the ✕ on a scheduled task) — ends its recurrence. */
export async function cancelPlanNode(node: number): Promise<void> {
  const res = await fetch(`/api/plan/${node}/cancel`, { method: "POST" });
  if (!res.ok) throw new Error(`cancel node: ${res.status}`);
}

/** Stop an agent run (the ✕ on a run row) — interrupts it if still live. */
export async function stopRun(id: string): Promise<void> {
  const res = await fetch(`/api/runs/${encodeURIComponent(id)}/stop`, {
    method: "POST",
  });
  if (!res.ok) throw new Error(`stop run: ${res.status}`);
}

async function errorText(res: Response, fallback: string): Promise<string> {
  return (await res.text().catch(() => "")).trim() || fallback;
}

/** Start dictation: the host machine begins capturing its own microphone. */
export async function startVoice(): Promise<void> {
  const res = await fetch("/api/voice/start", { method: "POST" });
  if (!res.ok) throw new Error(await errorText(res, `voice start: ${res.status}`));
}

/** Stop dictation: the host transcribes the capture and returns the text. */
export async function stopVoice(): Promise<string> {
  const res = await fetch("/api/voice/stop", { method: "POST" });
  if (!res.ok) throw new Error(await errorText(res, `voice stop: ${res.status}`));
  const body = (await res.json()) as { text: string };
  return body.text;
}

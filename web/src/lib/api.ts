/** REST surface of the gateway: session history for the picker and replay. */

import type { ContentBlock, ToolCall } from "./types";

export interface SessionSummary {
  id: string;
  title: string;
  updated_at: number; // unix milliseconds
}

export type ReplayEvent =
  | { type: "user"; seq: number; text: string; parts?: ContentBlock[] }
  | { type: "assistant"; seq: number; text?: string; tool_calls?: ToolCall[] }
  | {
      type: "tool_result";
      seq: number;
      call_id: string;
      content?: string;
      is_error?: boolean;
      details?: unknown;
    }
  | { type: "interrupted"; seq: number };

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

/** One slash command (a prompt template) the composer's popup offers. */
export interface SlashCommand {
  name: string;
  description?: string;
  argument_hint?: string;
  /** The template's source file, for the popup's edit affordance. */
  path?: string;
}

/** The slash commands available to this host (expansion stays server-side). */
export async function fetchCommands(): Promise<SlashCommand[]> {
  const res = await fetch("/api/commands");
  if (!res.ok) throw new Error(`commands: ${res.status}`);
  const body = (await res.json()) as { commands?: SlashCommand[] };
  return body.commands ?? [];
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

/** One poll of a job terminal tab: current status plus the journal bytes
 * since `offset` (base64 — process output is not guaranteed UTF-8). */
export interface JobTail {
  id: string;
  command: string;
  cwd: string;
  status: "running" | "exited" | "killed";
  exit_code: number;
  data: string; // base64
  offset: number; // next poll's offset
  size: number;
}

/** Tail a background job's journal. offset −1 starts at the recent window. */
export async function fetchJobTail(id: string, offset: number): Promise<JobTail> {
  const res = await fetch(
    `/api/jobs/${encodeURIComponent(id)}/tail?offset=${offset}`,
  );
  if (!res.ok) {
    throw new HttpError(await errorText(res, `job tail: ${res.status}`), res.status);
  }
  return (await res.json()) as JobTail;
}

/** An interactive shell the gateway spawned for a terminal tab. */
export interface TerminalInfo {
  id: string;
  cwd: string;
}

/** Spawn a shell on the host (optionally in a specific cwd) for a new
 * terminal tab; the tab attaches to its byte stream over /ws. */
export async function createTerminal(cwd?: string): Promise<TerminalInfo> {
  const res = await fetch("/api/terminals", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(cwd ? { cwd } : {}),
  });
  if (!res.ok) throw new Error(await errorText(res, `terminal: ${res.status}`));
  return (await res.json()) as TerminalInfo;
}

/** Hang up a shell (the terminal tab closed). */
export async function closeTerminal(id: string): Promise<void> {
  await fetch(`/api/terminals/${encodeURIComponent(id)}`, { method: "DELETE" });
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

/** A workspace file as the surface viewers read it through the gateway. */
export interface FileInfo {
  path: string;
  mtime: number; // unix milliseconds
  size: number;
  content?: string; // absent for stat-only reads and binary files
  truncated?: boolean;
  binary?: boolean;
}

/** A failed gateway request, with the HTTP status for callers that branch on
 * it (the prompt editor treats a 404 as "new file", not an error). */
export class HttpError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message);
  }
}

/**
 * Read a workspace file for a surface panel. `statOnly` skips the content —
 * the cheap change-poll that keeps an open panel live.
 */
export async function fetchFile(path: string, statOnly = false): Promise<FileInfo> {
  const qs = new URLSearchParams({ path });
  if (statOnly) qs.set("stat", "1");
  const res = await fetch(`/api/file?${qs}`);
  if (!res.ok) {
    throw new HttpError(await errorText(res, `file: ${res.status}`), res.status);
  }
  return (await res.json()) as FileInfo;
}

/**
 * Write a text file back through the gateway (the prompt editor's save).
 * Parent directories are created server-side; returns the fresh stat so the
 * editor's change-poll baseline is its own write.
 */
export async function writeFile(path: string, content: string): Promise<FileInfo> {
  const res = await fetch("/api/file", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ path, content }),
  });
  if (!res.ok) {
    throw new HttpError(await errorText(res, `write: ${res.status}`), res.status);
  }
  return (await res.json()) as FileInfo;
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

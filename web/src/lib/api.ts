/** REST surface of the gateway: session history for the picker and replay. */

import type { Citation, ContentBlock, ToolCall } from "./types";

export interface SessionSummary {
  id: string;
  title: string;
  updated_at: number; // unix milliseconds
}

export type ReplayEvent =
  | { type: "user"; seq: number; text: string; parts?: ContentBlock[]; author?: string }
  | {
      type: "assistant";
      seq: number;
      text?: string;
      /** Provider-generated images persisted on the assistant message. */
      parts?: ContentBlock[];
      tool_calls?: ToolCall[];
      citations?: Citation[];
    }
  | {
      type: "tool_result";
      seq: number;
      call_id: string;
      content?: string;
      is_error?: boolean;
      details?: unknown;
    }
  | { type: "interrupted"; seq: number }
  | { type: "chat_note"; seq: number; text: string; author?: string }
  | {
      type: "branch_anchor";
      seq: number;
      branch: string;
      anchor_seq: number;
      anchor_start?: number;
      anchor_end?: number;
      quote: string;
      branch_status: "open" | "accepted" | "discarded";
      summary?: string;
    };

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

/** The catalog request shared by every consumer (one fetch per page load):
 *  each chat tab and picker used to fire its own identical /api/models. */
let modelsPromise: Promise<ModelCatalog> | null = null;

export function fetchModels(): Promise<ModelCatalog> {
  if (!modelsPromise) {
    modelsPromise = (async () => {
      const res = await fetch("/api/models");
      if (!res.ok) throw new Error(`models: ${res.status}`);
      const body = (await res.json()) as {
        models?: ModelOption[];
        current?: string;
      };
      return { models: body.models ?? [], current: body.current ?? "" };
    })().catch((e: unknown) => {
      modelsPromise = null; // let a later caller retry a failed fetch
      throw e;
    });
  }
  return modelsPromise;
}

/** Drop the cached catalog so the next fetchModels hits the network. Used
 *  after a provider change, whose re-exec gives the host a real models
 *  endpoint where it had none before. */
export function resetModelsCache(): void {
  modelsPromise = null;
}

/** One slash command (a prompt template) the composer's popup offers. */
export interface SlashCommand {
  name: string;
  description?: string;
  argument_hint?: string;
  /** The template's source file, for the popup's edit affordance. */
  path?: string;
  /** The full template text, sent only for built-ins (no file on disk) so the
   * editor can open the shipped definition for in-place editing. */
  content?: string;
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

/** Durable per-session control state a resumed tab seeds its composer from. */
export interface SessionMeta {
  model?: string;
  web_search: boolean;
  web_fetch: boolean;
  image_gen: boolean;
}

export async function fetchReplay(sessionId: string): Promise<ReplayEvent[]> {
  return (await fetchReplayFull(sessionId)).events;
}

/** Replay plus the session record's control state (model, provider toggles). */
export async function fetchReplayFull(
  sessionId: string,
): Promise<{ events: ReplayEvent[]; session?: SessionMeta }> {
  const res = await fetch(`/api/sessions/${encodeURIComponent(sessionId)}/events`);
  if (!res.ok) throw new Error(`events: ${res.status}`);
  return (await res.json()) as { events: ReplayEvent[]; session?: SessionMeta };
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

/** One execution of a plan node — its verdict and what it learned. */
export interface PlanAttempt {
  verdict: string;
  outcome?: string;
  session?: string;
  at: number; // unix milliseconds
}

/**
 * One goal in a plan's tree, with the full definition the detail view renders:
 * the goal text, how it's checked, and the "code" that discharges it — the
 * shell command (`cmd`), gate predicate (`cond`), or notify payload — plus the
 * node's attempt history.
 */
export interface PlanNode {
  node: number;
  parent?: number;
  seq: number;
  goal: string;
  check?: string;
  cmd?: string;
  cond?: string;
  notify?: string;
  executor: "shell" | "notify" | "agent" | "ask";
  status: string;
  when?: string;
  outcome?: string;
  assignee?: string;
  attempts?: PlanAttempt[];
}

/** A whole plan: its root goal as a title plus every node's definition. */
export interface Plan {
  plan: number;
  title: string;
  chat?: string;
  nodes: PlanNode[];
}

/** Fetch the whole plan a node belongs to (the goal tree + each node's code). */
export async function fetchPlan(node: number): Promise<Plan> {
  const res = await fetch(`/api/plan/${node}`);
  if (!res.ok) throw new Error(`plan: ${res.status}`);
  return (await res.json()) as Plan;
}

/** One live browser tab: its id, screencast stream, and last known URL. */
export interface BrowserTab {
  id: string;
  stream: string;
  url?: string;
  active?: boolean;
}

/** The open browser tabs (empty when the browser isn't running). */
export async function listBrowserTabs(): Promise<BrowserTab[]> {
  const res = await fetch("/api/browser/tabs");
  if (!res.ok) throw new Error(`browser tabs: ${res.status}`);
  const body = (await res.json()) as { tabs: BrowserTab[] };
  return body.tabs ?? [];
}

/** Open a fresh browser tab (launching the browser if needed). */
export async function createBrowserTab(): Promise<BrowserTab> {
  const res = await fetch("/api/browser/tabs", { method: "POST" });
  if (!res.ok) throw new Error(`new browser tab: ${res.status}`);
  return (await res.json()) as BrowserTab;
}

/** Close one browser tab (the Chrome tab, not just its panel). */
export async function closeBrowserTab(id: string): Promise<void> {
  const res = await fetch(`/api/browser/tabs/${encodeURIComponent(id)}`, {
    method: "DELETE",
  });
  if (!res.ok) throw new Error(`close browser tab: ${res.status}`);
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

/** One row of a directory listing (the browser panel). */
export interface DirEntry {
  name: string;
  dir?: boolean;
  size?: number;
  mtime?: number; // unix milliseconds
}

/** A workspace file as the surface viewers read it through the gateway. */
export interface FileInfo {
  path: string;
  mtime: number; // unix milliseconds
  size: number;
  content?: string; // absent for stat-only reads and binary files
  truncated?: boolean;
  binary?: boolean;
  /** A directory: content is `entries` (the browser's listing) instead. */
  dir?: boolean;
  entries?: DirEntry[];
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

/**
 * Spool a binary file (base64, no data: prefix) back through the gateway,
 * decoded server-side so a PDF attachment lands intact rather than as mangled
 * UTF-8. Same response shape as writeFile.
 */
export async function writeFileBase64(path: string, base64: string): Promise<FileInfo> {
  const res = await fetch("/api/file", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ path, content: base64, encoding: "base64" }),
  });
  if (!res.ok) {
    throw new HttpError(await errorText(res, `write: ${res.status}`), res.status);
  }
  return (await res.json()) as FileInfo;
}

async function errorText(res: Response, fallback: string): Promise<string> {
  return (await res.text().catch(() => "")).trim() || fallback;
}

/**
 * One managed secret's metadata — never its value. The value is write-only: it
 * is sent on save and never returned, so the panel can show that a secret
 * exists (and edit its label/hosts/scope) but never read it back.
 */
export interface SecretMeta {
  /** Environment-variable name and the reference the agent uses. */
  name: string;
  /** Human label shown in the UI. */
  label?: string;
  /** Destination host allowlist for HTTP brokering (anti-exfiltration). */
  hosts?: string[];
  /** Whether the secret is injected into tool subprocess environments. */
  env?: boolean;
  /** Header the brokered value is injected into. Empty = Authorization/Bearer. */
  header?: string;
  /** Value prefix inside that header (e.g. "Bearer "). Empty = raw value. */
  prefix?: string;
}

/** The fields a save sends: metadata plus an optional value (omit to keep). */
export interface SecretUpsert {
  name: string;
  label?: string;
  value?: string;
  hosts?: string[];
  env?: boolean;
  header?: string;
  prefix?: string;
}

/** List the managed secrets' metadata (no values) for the Settings panel. */
export async function fetchSecrets(): Promise<SecretMeta[]> {
  const res = await fetch("/api/secrets");
  if (!res.ok) throw new Error(await errorText(res, `secrets: ${res.status}`));
  const body = (await res.json()) as { secrets?: SecretMeta[] };
  return body.secrets ?? [];
}

/**
 * Create or update a secret. An empty/absent `value` edits only metadata and
 * keeps the stored value, so editing a label never forces re-pasting the key.
 */
export async function saveSecret(s: SecretUpsert): Promise<void> {
  const res = await fetch(`/api/secrets/${encodeURIComponent(s.name)}`, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      label: s.label ?? "",
      value: s.value ?? "",
      hosts: s.hosts ?? [],
      env: s.env ?? false,
      header: s.header ?? "",
      prefix: s.prefix ?? "",
    }),
  });
  if (!res.ok) throw new Error(await errorText(res, `save secret: ${res.status}`));
}

/** Delete a secret and its value. */
export async function deleteSecret(name: string): Promise<void> {
  const res = await fetch(`/api/secrets/${encodeURIComponent(name)}`, {
    method: "DELETE",
  });
  if (!res.ok) throw new Error(await errorText(res, `delete secret: ${res.status}`));
}

/**
 * Durable host preferences — the agent knobs the Go host reads (unlike the
 * cosmetic localStorage settings, which never leave this browser). The whole
 * object travels both ways: load it, edit a field, save it back.
 */
export interface HostSettings {
  /** Model new sessions start on; empty follows the launch environment
   * (ARBOS_MODEL / provider default). Also written by the agent's own
   * set_model tool. Applies at the next arbos start. */
  default_model?: string;
  /** Model delegated sub-agents run on; empty follows the main model. */
  subagent_model?: string;
  /** Stored LLM endpoint; edited through the provider panel (/api/llm),
   * carried here so the wholesale settings PUT round-trips it intact. */
  llm_base_url?: string;
}

/** Load the host preference file for the Settings panel. */
export async function fetchHostSettings(): Promise<HostSettings> {
  const res = await fetch("/api/settings");
  if (!res.ok) throw new Error(await errorText(res, `settings: ${res.status}`));
  return (await res.json()) as HostSettings;
}

/** Persist the host preference file (applies without restarting arbos). */
export async function saveHostSettings(s: HostSettings): Promise<void> {
  const res = await fetch("/api/settings", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(s),
  });
  if (!res.ok) throw new Error(await errorText(res, `save settings: ${res.status}`));
}

/**
 * The host's LLM provider picture for the Settings panel: the effective
 * endpoint after env/stored resolution, whether any key resolves (the value
 * itself is write-only, like a secret), and whether the endpoint is
 * OpenRouter — which is what makes credits available.
 */
export interface LLMInfo {
  endpoint: string;
  provider: string;
  model: string;
  key_set: boolean;
  openrouter: boolean;
  /** Names the host process instance; changes exactly when the apply restart
   * lands, so a save can be confirmed by polling for a new value. */
  boot_id: string;
  /** True while a saved change waits for the agent to go idle. */
  restart_pending: boolean;
}

/** The fields a provider save sends — only what changed. The key is
 * write-only; an empty endpoint resets to the default. */
export interface LLMUpdate {
  endpoint?: string;
  key?: string;
}

/** Account credit totals (USD) as OpenRouter reports them. */
export interface LLMCredits {
  total_credits: number;
  total_usage: number;
}

/** Load the provider configuration for the Settings panel. */
export async function fetchLLM(): Promise<LLMInfo> {
  const res = await fetch("/api/llm");
  if (!res.ok) throw new Error(await errorText(res, `llm: ${res.status}`));
  return (await res.json()) as LLMInfo;
}

/**
 * Persist endpoint and/or key. The host applies the change by gracefully
 * restarting itself at the next idle moment — open tabs reconnect on their
 * own a few seconds later.
 */
export async function saveLLM(update: LLMUpdate): Promise<void> {
  const res = await fetch("/api/llm", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(update),
  });
  if (!res.ok) throw new Error(await errorText(res, `save llm: ${res.status}`));
}

/** Fetch credit totals; the key is attached server-side and never travels. */
export async function fetchLLMCredits(): Promise<LLMCredits> {
  const res = await fetch("/api/llm/credits");
  if (!res.ok) throw new Error(await errorText(res, `credits: ${res.status}`));
  return (await res.json()) as LLMCredits;
}

/** What a scoped share link points at: one artifact in its own namespace, or
 *  the whole agent ("all", no ref). */
export interface ShareScope {
  kind: "file" | "session" | "trajectory" | "all";
  /** Workspace path for a file artifact; session id for a chat or its
   *  trajectory snapshot; "" for all. */
  ref: string;
}

/** How much a share link grants within its scope. Mirrors share.Perm; the
 *  cells the backend enforces today are read everywhere and write on a chat. */
export type SharePerm = "read" | "write" | "admin";

/**
 * Mint a scoped, read-only share link for a single artifact — narrower than
 * createShareLink's full-agent invite. ttlSeconds of 0 means no expiry (capped
 * server-side). Returns the /s/<token> URL. Fails (404) on hosts without an
 * auth gate, where there's nothing to gate a link against.
 */
export async function shareArtifact(
  scope: ShareScope,
  ttlSeconds: number,
  perm: SharePerm = "read",
): Promise<string> {
  const res = await fetch("/api/share/link", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ scope, perm, ttl_seconds: ttlSeconds }),
  });
  if (!res.ok) throw new Error(await errorText(res, `share: ${res.status}`));
  const body = (await res.json()) as { url: string };
  return body.url;
}

/** The current principal's capabilities. A full login (or a full-agent share)
 *  is "local" — the whole workspace; a session-scoped share is "share", which
 *  the SPA renders as just that one chat, read-only or writable. */
export interface Me {
  kind: "local" | "share";
  scope?: string;
  session?: string;
  perm?: SharePerm;
  /** A share guest's self-asserted display name, so the SPA can label the
   *  guest's own messages live (the server stamps it on the wire regardless). */
  name?: string;
}

export async function fetchMe(): Promise<Me> {
  const res = await fetch("/api/me");
  if (!res.ok) throw new Error(`me: ${res.status}`);
  return (await res.json()) as Me;
}

/** Revoke a scoped link by its token (the /s/<token> tail), cascading to any
 *  links delegated from it. Revoking an unknown token succeeds (idempotent). */
export async function revokeShare(token: string): Promise<void> {
  const res = await fetch(`/api/share/link/${encodeURIComponent(token)}`, {
    method: "DELETE",
  });
  if (!res.ok && res.status !== 204) {
    throw new Error(await errorText(res, `revoke: ${res.status}`));
  }
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

/**
 * Transcribe a recorded clip (browser mic capture, voice-memo attachment)
 * through the provider's speech-to-text. data is bare base64; format is the
 * container (webm, m4a, mp3, wav, ogg, …).
 */
export async function transcribeAudio(data: string, format: string): Promise<string> {
  const res = await fetch("/api/voice/transcribe", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ data, format }),
  });
  if (!res.ok) throw new Error(await errorText(res, `transcribe: ${res.status}`));
  const body = (await res.json()) as { text: string };
  return body.text;
}

/** Synthesize speech for the given text; resolves to playable MP3 bytes. */
export async function fetchSpeech(text: string): Promise<Blob> {
  const res = await fetch("/api/tts", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ text }),
  });
  if (!res.ok) throw new Error(await errorText(res, `tts: ${res.status}`));
  return res.blob();
}

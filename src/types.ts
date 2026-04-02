import type { TextChannel, ThreadChannel, Message } from "discord.js";

// ── Config ──────────────────────────────────────────────────────────────────

export interface Config {
  discordToken: string;
  guildId: string;
  openRouterKey: string;
  workspaceRoot: string;
  vaultKey: string;
}

// ── Chat Memory ─────────────────────────────────────────────────────────────

export interface ChatEntry {
  ts: string;
  role: "user" | "assistant" | "system";
  author?: string;
  content: string;
}

// ── Agent ───────────────────────────────────────────────────────────────────

export interface AgentResult {
  output: string;
  stderr: string;
  exitCode: number;
  durationMs: number;
  promptLength: number;
}

export type StreamCallback = (chunk: string, full: string) => void;

// ── RALPH Loop ──────────────────────────────────────────────────────────────

export type LoopState = "idle" | "running" | "paused" | "sleeping" | "error" | "closed";

export interface RalphMeta {
  state: LoopState;
  delayMinutes: number;
  lastStepAt: string;
  errorCount: number;
  stepCount: number;
  currentStep?: number;
}

// ── Discord Queue ───────────────────────────────────────────────────────────

export interface StreamSession {
  channelId: string;
  messageIds: string[];
  buffer: string;
  lastFlush: number;
  done: boolean;
}

// ── Workspace Resolution ────────────────────────────────────────────────────

export interface ResolvedContext {
  cwd: string;              // agent working directory
  chatDir: string;          // where chat logs live
  pinPath: string;          // PIN.md location
  isThread: boolean;
  threadDir?: string;       // threads/<name>/ if thread
  parentChannelName: string;
}

// ── Discord helpers ─────────────────────────────────────────────────────────

export type AnyTextChannel = TextChannel | ThreadChannel;

export function isThread(channel: AnyTextChannel): channel is ThreadChannel {
  return channel.isThread();
}

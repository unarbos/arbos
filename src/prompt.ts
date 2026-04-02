import { resolve } from "path";
import type { Config, ResolvedContext, ChatEntry } from "./types.js";
import {
  readPin,
  fillPlaceholders,
  buildPlaceholderVars,
  getRecentChat,
  formatChat,
  getSummary,
  readGoal,
  readState,
} from "./workspace.js";
import { getManifest } from "./vault.js";

const CHANNEL_RECENT = 15;
const THREAD_RECENT = 10;
const MAX_ENTRY_CHARS = 4_000;
const PROMPT_BUDGET = 100_000;

function truncateEntries(entries: ChatEntry[]): ChatEntry[] {
  return entries.map((e) => {
    if (e.content.length <= MAX_ENTRY_CHARS) return e;
    return { ...e, content: e.content.slice(0, MAX_ENTRY_CHARS) + "\n…(truncated)" };
  });
}

/**
 * Enforce a total character budget by dropping lowest-priority sections first.
 * Priority (highest to lowest): pin, goal, state, chat, summary.
 * Instructions (last part) and pin (first part) are always kept.
 */
function trimToFit(parts: string[], budget: number = PROMPT_BUDGET): string {
  let prompt = parts.join("\n\n");
  if (prompt.length <= budget) return prompt;

  const instructions = parts[parts.length - 1];
  const pin = parts[0];
  const middle = parts.slice(1, -1);

  const dropOrder = [
    "Thread summary:",
    "Channel summary:",
    "Recent thread chat:",
    "Recent chat:",
    "State:",
  ];

  for (const prefix of dropOrder) {
    const idx = middle.findIndex((p) => p.startsWith(prefix));
    if (idx !== -1) {
      middle.splice(idx, 1);
      const joined = [pin, ...middle, instructions].join("\n\n");
      if (joined.length <= budget) return joined;
    }
  }

  prompt = [pin, ...middle, instructions].join("\n\n");
  if (prompt.length <= budget) return prompt;

  const overhead = pin.length + instructions.length + 4;
  const available = budget - overhead;
  const middleStr = middle.join("\n\n");
  return [pin, middleStr.slice(0, available) + "\n…(prompt truncated)", instructions].join("\n\n");
}

export async function buildChannelPrompt(
  config: Config,
  ctx: ResolvedContext
): Promise<string> {
  const [rawPin, recent, summary, envManifest] = await Promise.all([
    readPin(ctx.pinPath),
    getRecentChat(ctx.chatDir, CHANNEL_RECENT),
    getSummary(ctx.chatDir),
    getManifest(),
  ]);

  const vars = buildPlaceholderVars(config, ctx.parentChannelName, { cwd: ctx.cwd });
  const pin = fillPlaceholders(rawPin, vars);
  const parts: string[] = [pin.trim()];

  if (envManifest) {
    parts.push(envManifest);
  }

  if (recent.length > 0) {
    parts.push(`Recent chat:\n${formatChat(truncateEntries(recent))}`);
  }

  if (summary) {
    parts.push(`Channel summary:\n${summary}`);
  }

  parts.push(
    [
      `You are Arbos. You are running inside:`,
      resolve(ctx.cwd),
      ``,
      `Respond to the latest message and use the workspace files as needed.`,
      `Be concise unless asked otherwise.`,
    ].join("\n")
  );

  return trimToFit(parts);
}

export async function buildThreadPrompt(
  config: Config,
  ctx: ResolvedContext
): Promise<string> {
  if (!ctx.threadDir) throw new Error("buildThreadPrompt called without threadDir");

  const [rawPin, goal, state, recent, summary, envManifest] = await Promise.all([
    readPin(ctx.pinPath),
    readGoal(ctx.threadDir),
    readState(ctx.threadDir),
    getRecentChat(ctx.chatDir, THREAD_RECENT),
    getSummary(ctx.chatDir),
    getManifest(),
  ]);

  const vars = buildPlaceholderVars(config, ctx.parentChannelName, {
    cwd: ctx.cwd, threadName: ctx.threadDir, threadDir: ctx.threadDir,
  });
  const pin = fillPlaceholders(rawPin, vars);
  const parts: string[] = [pin.trim()];

  if (envManifest) {
    parts.push(envManifest);
  }

  if (goal) {
    parts.push(`Goal:\n${goal.trim()}`);
  }

  if (state) {
    parts.push(`State:\n${state.trim()}`);
  }

  if (recent.length > 0) {
    parts.push(`Recent thread chat:\n${formatChat(truncateEntries(recent))}`);
  }

  if (summary) {
    parts.push(`Thread summary:\n${summary}`);
  }

  parts.push(
    [
      `You are Arbos running a continuous loop for this thread.`,
      `Your working directory is: ${resolve(ctx.cwd)}`,
      `Thread data is in: ${resolve(ctx.threadDir)}`,
      ``,
      `Advance the goal by one meaningful step.`,
      `Update STATE.md with durable progress.`,
      `Explain what you changed and what comes next.`,
    ].join("\n")
  );

  return trimToFit(parts);
}

/**
 * Build a one-shot prompt for a thread where the user sent a manual message
 * (not an automatic RALPH step).
 */
export async function buildThreadChatPrompt(
  config: Config,
  ctx: ResolvedContext
): Promise<string> {
  if (!ctx.threadDir) throw new Error("buildThreadChatPrompt called without threadDir");

  const [rawPin, goal, state, recent, summary, envManifest] = await Promise.all([
    readPin(ctx.pinPath),
    readGoal(ctx.threadDir),
    readState(ctx.threadDir),
    getRecentChat(ctx.chatDir, THREAD_RECENT),
    getSummary(ctx.chatDir),
    getManifest(),
  ]);

  const vars = buildPlaceholderVars(config, ctx.parentChannelName, {
    cwd: ctx.cwd, threadName: ctx.threadDir, threadDir: ctx.threadDir,
  });
  const pin = fillPlaceholders(rawPin, vars);
  const parts: string[] = [pin.trim()];

  if (envManifest) {
    parts.push(envManifest);
  }

  if (goal) {
    parts.push(`Goal:\n${goal.trim()}`);
  }

  if (state) {
    parts.push(`State:\n${state.trim()}`);
  }

  if (recent.length > 0) {
    parts.push(`Recent thread chat:\n${formatChat(truncateEntries(recent))}`);
  }

  if (summary) {
    parts.push(`Thread summary:\n${summary}`);
  }

  parts.push(
    [
      `You are Arbos. You are running inside:`,
      resolve(ctx.cwd),
      `Thread data is in: ${resolve(ctx.threadDir)}`,
      ``,
      `Respond to the latest message. Use workspace files as needed.`,
      `Be concise unless asked otherwise.`,
    ].join("\n")
  );

  return trimToFit(parts);
}

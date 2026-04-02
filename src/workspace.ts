import { mkdir, readFile, readdir, writeFile, appendFile, rm, stat } from "fs/promises";
import { join, resolve } from "path";
import type { ChatEntry, RalphMeta, Config, ResolvedContext } from "./types.js";

const RECENT_WINDOW = 15;
const THREAD_RECENT_WINDOW = 10;
const SUMMARY_INTERVAL = 20;

// Per-workspace message counter for triggering summary regeneration
const msgCounters = new Map<string, number>();

// ── PIN Templates ───────────────────────────────────────────────────────────

const CHANNEL_PIN = `# PIN
- You are Arbos, an agent running as a Node.js process managed by pm2.
- Your operator sends you messages through a discord channel. 
- Your current channel is {{channel}}
- Each message spawns a \`claude\` CLI invocation.
- Each invocation is given workspace, yours is {{workspace}}.
- Each channels gets a workspace which is a sibling of your current.
- This channel's chat history is stored in \`/chat/raw.ndjson\`.
- Your Full chat log is a list of (role, author, content, timestamp) events.
- There is also a \`chat/summary.md\` which auto generates every 30 seconds.
- Your response streams back to Discord in real time.
- The code that runs you is two directories up in {{root}}
- You can edit your code but note this will trigger and auto restart.
- Threads are Ralph loops which are stored in \`/threads/<threadName>/\`
`;

const THREAD_PIN = `# PIN
You are Arbos running a continuous goal loop.
Use GOAL.md as the objective, STATE.md as durable progress memory.
Per step: inspect state, take one action, update STATE.md, report briefly.
`;

// ── Placeholder Substitution ─────────────────────────────────────────────────

export function fillPlaceholders(
  text: string,
  vars: Record<string, string>
): string {
  return text.replace(/\{\{(\w+)\}\}/g, (match, key) => vars[key] ?? match);
}

export function buildPlaceholderVars(
  config: Config,
  channelName: string,
  opts?: { threadName?: string; threadDir?: string; cwd?: string }
): Record<string, string> {
  return {
    channel: channelName,
    workspace: resolve(join(config.workspaceRoot, channelName)),
    root: resolve(config.workspaceRoot, ".."),
    thread: opts?.threadName ?? "",
    threadDir: opts?.threadDir ? resolve(opts.threadDir) : "",
    cwd: opts?.cwd ? resolve(opts.cwd) : resolve(join(config.workspaceRoot, channelName)),
  };
}

// ── Pinned Message Formatter ─────────────────────────────────────────────────

export function formatPinnedMessage(opts: {
  cwd: string;
  channelName: string;
  threadName?: string;
  threadDir?: string;
  goal?: string;
  ralph?: string;
  pin?: string;
}): string {
  const lines: string[] = [];

  const heading = opts.threadName
    ? `Arbos — #${opts.channelName} / ${opts.threadName}`
    : `Arbos — #${opts.channelName}`;

  lines.push(heading);
  lines.push(`CWD: ${resolve(opts.cwd)}`);

  if (opts.threadDir) {
    lines.push(`Thread dir: ${resolve(opts.threadDir)}`);
  }

  if (opts.pin) {
    lines.push("");
    lines.push("--- PIN ---");
    lines.push(opts.pin.trim());
  }

  if (opts.ralph) {
    lines.push("");
    lines.push("--- RALPH ---");
    lines.push(opts.ralph.trim());
  }

  if (opts.goal) {
    lines.push("");
    lines.push("--- GOAL ---");
    lines.push(opts.goal.trim());
  }

  return lines.join("\n");
}

// ── Directory Management ────────────────────────────────────────────────────

export async function ensureWorkspace(config: Config, channelName: string): Promise<string> {
  const dir = join(config.workspaceRoot, channelName);
  const chatDir = join(dir, "chat");
  await mkdir(chatDir, { recursive: true });

  const pinPath = join(dir, "PIN.md");
  if (!(await exists(pinPath))) {
    await writeFile(pinPath, CHANNEL_PIN);
  }

  return dir;
}

export async function ensureThread(
  config: Config,
  channelName: string,
  threadName: string,
  goal: string
): Promise<string> {
  await ensureWorkspace(config, channelName);

  const threadDir = join(config.workspaceRoot, channelName, "threads", threadName);
  const chatDir = join(threadDir, "chat");
  await mkdir(chatDir, { recursive: true });

  const pinPath = join(threadDir, "PIN.md");
  if (!(await exists(pinPath))) {
    await writeFile(pinPath, THREAD_PIN);
  }

  await writeFile(join(threadDir, "GOAL.md"), goal);

  const statePath = join(threadDir, "STATE.md");
  if (!(await exists(statePath))) {
    await writeFile(statePath, "# State\n\nNo progress yet.\n");
  }

  const ralphPath = join(threadDir, ".ralph.json");
  if (!(await exists(ralphPath))) {
    const meta: RalphMeta = {
      state: "idle",
      delayMinutes: 1,
      lastStepAt: "",
      errorCount: 0,
      stepCount: 0,
    };
    await writeFile(ralphPath, JSON.stringify(meta, null, 2));
  }

  return threadDir;
}

// ── Per-Channel CWD Override ─────────────────────────────────────────────────

export async function readChannelCwd(config: Config, channelName: string): Promise<string | null> {
  const cwdFile = join(config.workspaceRoot, channelName, ".cwd");
  const raw = await safeRead(cwdFile);
  return raw ? raw.trim() : null;
}

export async function writeChannelCwd(config: Config, channelName: string, cwdPath: string): Promise<void> {
  const cwdFile = join(config.workspaceRoot, channelName, ".cwd");
  await writeFile(cwdFile, cwdPath);
}

export async function resolveChannelCwd(config: Config, channelName: string): Promise<string> {
  const override = await readChannelCwd(config, channelName);
  if (override) {
    const projectRoot = resolve(config.workspaceRoot, "..");
    return resolve(projectRoot, override);
  }
  return resolve(join(config.workspaceRoot, channelName));
}

// ── Channel Topic ───────────────────────────────────────────────────────────

export function buildChannelTopic(cwd: string): string {
  return cwd;
}

// ── Context Resolution ──────────────────────────────────────────────────────

export async function resolveContext(
  config: Config,
  channelName: string,
  threadName?: string
): Promise<ResolvedContext> {
  const channelDir = join(config.workspaceRoot, channelName);
  const cwd = await resolveChannelCwd(config, channelName);

  if (threadName) {
    const threadDir = join(channelDir, "threads", threadName);
    return {
      cwd,
      chatDir: join(threadDir, "chat"),
      pinPath: join(threadDir, "PIN.md"),
      isThread: true,
      threadDir,
      parentChannelName: channelName,
    };
  }

  return {
    cwd,
    chatDir: join(channelDir, "chat"),
    pinPath: join(channelDir, "PIN.md"),
    isThread: false,
    parentChannelName: channelName,
  };
}

// ── Chat Memory: Tier 1 - Raw Log ──────────────────────────────────────────

export async function appendChat(chatDir: string, entry: ChatEntry): Promise<void> {
  await mkdir(chatDir, { recursive: true });
  const line = JSON.stringify(entry) + "\n";
  await appendFile(join(chatDir, "raw.ndjson"), line);
}

// ── Chat Memory: Tier 2 - Recent Window ─────────────────────────────────────

export async function getRecentChat(chatDir: string, limit?: number): Promise<ChatEntry[]> {
  const n = limit ?? RECENT_WINDOW;
  const raw = await safeRead(join(chatDir, "raw.ndjson"));
  if (!raw) return [];

  const lines = raw.trim().split("\n").filter(Boolean);
  const recent = lines.slice(-n);
  return recent.map((l) => JSON.parse(l) as ChatEntry);
}

export function formatChat(entries: ChatEntry[]): string {
  return entries
    .map((e) => {
      const prefix = e.role === "user" ? `${e.author ?? "user"}` : "Arbos";
      return `[${prefix}]: ${e.content}`;
    })
    .join("\n");
}

// ── Chat Memory: Tier 3 - Summary ───────────────────────────────────────────

export async function getSummary(chatDir: string): Promise<string> {
  return (await safeRead(join(chatDir, "summary.md"))) ?? "";
}

export async function shouldRegenSummary(chatDir: string): Promise<boolean> {
  const key = chatDir;
  const count = (msgCounters.get(key) ?? 0) + 1;
  msgCounters.set(key, count);
  return count % SUMMARY_INTERVAL === 0;
}

export async function regenerateSummary(
  chatDir: string,
  openRouterKey: string
): Promise<void> {
  try {
    const raw = await safeRead(join(chatDir, "raw.ndjson"));
    if (!raw) return;

    const lines = raw.trim().split("\n").filter(Boolean);
    const entries: ChatEntry[] = lines.map((l) => JSON.parse(l));
    const text = entries
      .map((e) => `[${e.role}${e.author ? ` (${e.author})` : ""}]: ${e.content}`)
      .join("\n");

    if (text.length < 200) return;

    const resp = await fetch("https://openrouter.ai/api/v1/chat/completions", {
      method: "POST",
      headers: {
        Authorization: `Bearer ${openRouterKey}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({
        model: "anthropic/claude-opus-4.6",
        messages: [
          {
            role: "system",
            content:
              "Summarize this conversation concisely. Focus on decisions made, actions taken, and current state. Max 500 words.",
          },
          { role: "user", content: text },
        ],
        max_tokens: 800,
      }),
    });

    if (!resp.ok) {
      console.error(`[summary] OpenRouter error: ${resp.status}`);
      return;
    }

    const data = (await resp.json()) as any;
    const summary = data.choices?.[0]?.message?.content ?? "";
    if (summary) {
      await writeFile(join(chatDir, "summary.md"), summary);
    }
  } catch (err) {
    console.error("[summary] Failed to regenerate:", err);
  }
}

// ── File Helpers ────────────────────────────────────────────────────────────

export async function readPin(pinPath: string): Promise<string> {
  const base = (await safeRead(pinPath)) ?? CHANNEL_PIN;
  const pinsDir = join(pinPath, "..", "pins");
  const extras = await readPinsDir(pinsDir);
  if (extras.length === 0) return base;
  return [base, ...extras].join("\n\n");
}

export async function readBasePin(pinPath: string): Promise<string> {
  return (await safeRead(pinPath)) ?? CHANNEL_PIN;
}

export async function appendPin(pinPath: string, content: string): Promise<number> {
  const pinsDir = join(pinPath, "..", "pins");
  await mkdir(pinsDir, { recursive: true });
  const existing = await safeReaddir(pinsDir);
  const nums = existing
    .map((f) => parseInt(f.replace(/\.md$/, ""), 10))
    .filter((n) => !isNaN(n));
  const next = nums.length > 0 ? Math.max(...nums) + 1 : 1;
  await writeFile(join(pinsDir, `${next}.md`), content);
  return next;
}

export async function writePin(pinPath: string, content: string): Promise<void> {
  await writeFile(pinPath, content);
}

export async function listPins(pinPath: string): Promise<{ id: number; content: string }[]> {
  const pinsDir = join(pinPath, "..", "pins");
  const entries = await safeReaddir(pinsDir);
  const pins: { id: number; content: string }[] = [];
  for (const entry of entries) {
    const num = parseInt(entry.replace(/\.md$/, ""), 10);
    if (isNaN(num)) continue;
    const content = await safeRead(join(pinsDir, entry));
    if (content) pins.push({ id: num, content });
  }
  return pins.sort((a, b) => a.id - b.id);
}

export async function clearPins(pinPath: string): Promise<number> {
  const pinsDir = join(pinPath, "..", "pins");
  const entries = await safeReaddir(pinsDir);
  let removed = 0;
  for (const entry of entries) {
    await rm(join(pinsDir, entry)).catch(() => {});
    removed++;
  }
  return removed;
}

async function readPinsDir(pinsDir: string): Promise<string[]> {
  const entries = await safeReaddir(pinsDir);
  const numbered = entries
    .map((f) => ({ name: f, num: parseInt(f.replace(/\.md$/, ""), 10) }))
    .filter((e) => !isNaN(e.num))
    .sort((a, b) => a.num - b.num);
  const contents: string[] = [];
  for (const entry of numbered) {
    const content = await safeRead(join(pinsDir, entry.name));
    if (content) contents.push(content);
  }
  return contents;
}

async function safeReaddir(path: string): Promise<string[]> {
  try {
    return await readdir(path);
  } catch {
    return [];
  }
}

export async function readGoal(threadDir: string): Promise<string> {
  return (await safeRead(join(threadDir, "GOAL.md"))) ?? "";
}

export async function writeGoal(threadDir: string, content: string): Promise<void> {
  await writeFile(join(threadDir, "GOAL.md"), content);
}

const STATE_MAX_CHARS = 20_000;

export async function readState(threadDir: string, maxChars?: number): Promise<string> {
  const raw = (await safeRead(join(threadDir, "STATE.md"))) ?? "# State\n\nNo progress yet.\n";
  const limit = maxChars ?? STATE_MAX_CHARS;
  if (raw.length <= limit) return raw;
  return "…(truncated)\n" + raw.slice(-limit);
}

export async function readRalphMeta(threadDir: string): Promise<RalphMeta> {
  const raw = await safeRead(join(threadDir, ".ralph.json"));
  if (!raw) {
    return { state: "idle", delayMinutes: 0, lastStepAt: "", errorCount: 0, stepCount: 0 };
  }
  const meta = JSON.parse(raw);
  if (meta.stepCount === undefined) meta.stepCount = 0;
  return meta;
}

export async function ensureStepDir(threadDir: string, stepNum: number): Promise<string> {
  const stepDir = join(threadDir, "steps", String(stepNum));
  await mkdir(stepDir, { recursive: true });
  return stepDir;
}

export async function writeRalphMeta(threadDir: string, meta: RalphMeta): Promise<void> {
  await writeFile(join(threadDir, ".ralph.json"), JSON.stringify(meta, null, 2));
}

// ── Thread Detection ────────────────────────────────────────────────────────

export async function threadIsInitialized(
  config: Config,
  channelName: string,
  threadName: string
): Promise<boolean> {
  const ralphPath = join(config.workspaceRoot, channelName, "threads", threadName, ".ralph.json");
  return exists(ralphPath);
}

// ── Utilities ───────────────────────────────────────────────────────────────

async function safeRead(path: string): Promise<string | null> {
  try {
    return await readFile(path, "utf-8");
  } catch {
    return null;
  }
}

async function exists(path: string): Promise<boolean> {
  try {
    await stat(path);
    return true;
  } catch {
    return false;
  }
}

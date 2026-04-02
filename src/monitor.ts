import {
  Client,
  ChannelType,
  TextChannel,
  type Guild,
} from "discord.js";
import type { Config } from "./types.js";
import { resolve } from "path";

export const MONITOR_CHANNEL_NAME = "logs";
const FLUSH_INTERVAL_MS = 10_000;
const MAX_MSG_LEN = 2000;

export type MonitorLevel = "info" | "warn" | "error" | "debug";

export interface MonitorEvent {
  ts: string;
  level: MonitorLevel;
  tag: string;
  msg: string;
  data?: Record<string, unknown>;
}

let monitorChannelId: string | null = null;
let monitorConfig: Config | null = null;
let monitorClient: Client | null = null;
let eventBuffer: MonitorEvent[] = [];
let flushTimer: ReturnType<typeof setInterval> | null = null;
let flushInProgress = false;

export async function initMonitor(
  client: Client,
  config: Config,
): Promise<void> {
  monitorConfig = config;
  monitorClient = client;

  try {
    await ensureChannel();
  } catch (err) {
    console.error("[monitor] Failed to create monitor channel — will retry on flush:", err);
  }

  flushTimer = setInterval(() => flushBuffer(), FLUSH_INTERVAL_MS);

  mlog("info", "boot", "Arbos monitor initialized", {
    cwd: resolve(config.workspaceRoot),
    pid: process.pid,
    nodeVersion: process.version,
    uptime: process.uptime(),
  });
}

export function destroyMonitor(): void {
  if (flushTimer) {
    clearInterval(flushTimer);
    flushTimer = null;
  }
  flushBuffer();
}

async function getGuild(): Promise<Guild | null> {
  if (!monitorClient || !monitorConfig) return null;
  return monitorClient.guilds.cache.get(monitorConfig.guildId) ?? null;
}

async function ensureChannel(): Promise<string | null> {
  const guild = await getGuild();
  if (!guild) return null;

  const channels = await guild.channels.fetch();
  for (const [id, ch] of channels) {
    if (ch && ch.type === ChannelType.GuildText && ch.name === MONITOR_CHANNEL_NAME) {
      monitorChannelId = id;
      return id;
    }
  }

  console.log(`[monitor] Creating #${MONITOR_CHANNEL_NAME}`);
  const created = await guild.channels.create({
    name: MONITOR_CHANNEL_NAME,
    type: ChannelType.GuildText,
    topic: `Arbos process monitor — CWD: ${resolve(monitorConfig!.workspaceRoot)}`,
    reason: "Arbos monitor channel auto-created",
  });

  monitorChannelId = created.id;
  return created.id;
}

async function verifyChannel(): Promise<boolean> {
  if (!monitorChannelId || !monitorClient) {
    return !!(await ensureChannel());
  }

  try {
    const ch = await monitorClient.channels.fetch(monitorChannelId);
    if (ch) return true;
  } catch {
    // channel was deleted or inaccessible
  }

  console.warn("[monitor] Channel gone — recreating");
  monitorChannelId = null;
  return !!(await ensureChannel());
}

/**
 * Central structured logging. Writes to stdout (PM2 logs) and buffers for Discord.
 */
export function mlog(
  level: MonitorLevel,
  tag: string,
  msg: string,
  data?: Record<string, unknown>
): void {
  const event: MonitorEvent = {
    ts: new Date().toISOString(),
    level,
    tag,
    msg,
    data,
  };

  const logLine = formatLogLine(event);
  if (level === "error") {
    console.error(logLine);
  } else if (level === "warn") {
    console.warn(logLine);
  } else {
    console.log(logLine);
  }

  eventBuffer.push(event);
}

function formatLogLine(e: MonitorEvent): string {
  const ts = e.ts.slice(11, 23); // HH:mm:ss.SSS
  const lvl = e.level === "info" ? "I" : e.level === "warn" ? "W" : e.level === "error" ? "E" : "D";
  const dataStr = e.data ? ` ${compactData(e.data)}` : "";
  return `${ts} ${lvl} [${e.tag}] ${e.msg}${dataStr}`;
}

function compactData(data: Record<string, unknown>): string {
  const parts: string[] = [];
  for (const [k, v] of Object.entries(data)) {
    if (v === null || v === undefined) continue;
    const val = typeof v === "string" ? v : JSON.stringify(v);
    parts.push(`${k}=${val}`);
  }
  return parts.join(" ");
}

/**
 * Flush buffered events to the Discord monitor channel as plain text.
 * Batches into code blocks, splitting across messages if needed.
 */
async function flushBuffer(): Promise<void> {
  if (flushInProgress || eventBuffer.length === 0 || !monitorClient) {
    return;
  }

  flushInProgress = true;

  try {
    const channelOk = await verifyChannel();
    if (!channelOk || !monitorChannelId) {
      flushInProgress = false;
      return;
    }

    const events = eventBuffer.splice(0);
    const lines = events.map(formatLogLine);

    const channel = await monitorClient.channels.fetch(monitorChannelId) as TextChannel;
    if (!channel) {
      flushInProgress = false;
      return;
    }

    // pack lines into code blocks that fit within Discord's 2000 char limit
    const codeBlockOverhead = "```\n".length + "\n```".length; // 8 chars
    const maxContent = MAX_MSG_LEN - codeBlockOverhead;
    let batch = "";

    for (const line of lines) {
      if (batch.length + line.length + 1 > maxContent) {
        await channel.send("```\n" + batch + "\n```");
        batch = "";
      }
      batch += (batch ? "\n" : "") + line;
    }

    if (batch) {
      await channel.send("```\n" + batch + "\n```");
    }
  } catch (err) {
    console.error("[monitor] Failed to flush to Discord:", err);
    monitorChannelId = null;
  } finally {
    flushInProgress = false;
  }
}

export async function forceFlush(): Promise<void> {
  await flushBuffer();
}

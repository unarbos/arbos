import type { Config, RalphMeta, ChatEntry, ResolvedContext } from "./types.js";
import { DiscordSendQueue } from "./queue.js";
import {
  readRalphMeta,
  writeRalphMeta,
  resolveContext,
  appendChat,
  shouldRegenSummary,
  regenerateSummary,
  ensureThread,
  ensureStepDir,
} from "./workspace.js";
import { buildThreadPrompt } from "./prompt.js";
import { runAgent, killAgent } from "./agent.js";
import { mlog } from "./monitor.js";
import { readdir, writeFile } from "fs/promises";
import { join } from "path";

interface ActiveLoop {
  threadId: string;
  channelName: string;
  threadName: string;
  parentChannelId: string;
  abortController: AbortController;
  startedAt: string;
}

const activeLoops = new Map<string, ActiveLoop>();

export function getActiveLoops(): Map<string, ActiveLoop> {
  return activeLoops;
}

// ── Loop Control ────────────────────────────────────────────────────────────

export async function startLoop(
  config: Config,
  queue: DiscordSendQueue,
  threadId: string,
  channelName: string,
  threadName: string,
  parentChannelId: string
): Promise<void> {
  if (activeLoops.has(threadId)) return;

  const threadDir = join(config.workspaceRoot, channelName, "threads", threadName);
  const meta = await readRalphMeta(threadDir);
  meta.state = "running";
  meta.errorCount = 0;
  await writeRalphMeta(threadDir, meta);

  const ac = new AbortController();
  activeLoops.set(threadId, {
    threadId,
    channelName,
    threadName,
    parentChannelId,
    abortController: ac,
    startedAt: new Date().toISOString(),
  });

  mlog("info", "ralph", `Loop started`, {
    threadId,
    channelName,
    threadName,
    parentChannelId,
    delayMinutes: meta.delayMinutes,
  });

  loopRunner(config, queue, threadId, channelName, threadName, parentChannelId, ac.signal);
}

export async function pauseLoop(
  config: Config,
  threadId: string,
  channelName: string,
  threadName: string
): Promise<void> {
  const loop = activeLoops.get(threadId);
  if (loop) {
    loop.abortController.abort();
    activeLoops.delete(threadId);
  }
  killAgent(threadId);

  const threadDir = join(config.workspaceRoot, channelName, "threads", threadName);
  const meta = await readRalphMeta(threadDir);
  mlog("info", "ralph", `Loop paused`, {
    threadId,
    channelName,
    threadName,
    stepsCompleted: meta.stepCount,
    runSince: loop?.startedAt,
  });
  meta.state = "paused";
  await writeRalphMeta(threadDir, meta);
}

export function stopLoop(threadId: string): boolean {
  const loop = activeLoops.get(threadId);
  if (!loop) return false;

  loop.abortController.abort();
  activeLoops.delete(threadId);
  killAgent(threadId);

  mlog("info", "ralph", `Loop force-stopped (workspace deleted)`, {
    threadId,
    channelName: loop.channelName,
    threadName: loop.threadName,
  });
  return true;
}

export function stopLoopsForChannel(channelName: string): number {
  let stopped = 0;
  for (const [threadId, loop] of activeLoops) {
    if (loop.channelName === channelName) {
      loop.abortController.abort();
      activeLoops.delete(threadId);
      killAgent(threadId);
      mlog("info", "ralph", `Loop force-stopped (channel deleted)`, {
        threadId,
        channelName: loop.channelName,
        threadName: loop.threadName,
      });
      stopped++;
    }
  }
  return stopped;
}

export async function setDelay(
  config: Config,
  channelName: string,
  threadName: string,
  minutes: number
): Promise<void> {
  const threadDir = join(config.workspaceRoot, channelName, "threads", threadName);
  const meta = await readRalphMeta(threadDir);
  meta.delayMinutes = minutes;
  await writeRalphMeta(threadDir, meta);
  mlog("info", "ralph", `Delay updated`, { channelName, threadName, minutes });
}

export async function runSingleStep(
  config: Config,
  queue: DiscordSendQueue,
  threadId: string,
  channelName: string,
  threadName: string
): Promise<void> {
  const threadDir = join(config.workspaceRoot, channelName, "threads", threadName);
  const meta = await readRalphMeta(threadDir);
  const stepNum = meta.stepCount + 1;
  meta.currentStep = stepNum;
  await writeRalphMeta(threadDir, meta);
  mlog("info", "ralph", `Manual single step triggered`, { channelName, threadName, stepNum });
  await executeStep(config, queue, threadId, channelName, threadName, stepNum);
  meta.stepCount = stepNum;
  meta.lastStepAt = new Date().toISOString();
  meta.currentStep = undefined;
  await writeRalphMeta(threadDir, meta);
}

// ── Loop Runner ─────────────────────────────────────────────────────────────

async function loopRunner(
  config: Config,
  queue: DiscordSendQueue,
  threadId: string,
  channelName: string,
  threadName: string,
  parentChannelId: string,
  signal: AbortSignal
) {
  const threadDir = join(config.workspaceRoot, channelName, "threads", threadName);

  while (!signal.aborted) {
    let meta: RalphMeta;
    try {
      meta = await readRalphMeta(threadDir);
    } catch (err: any) {
      if (err?.code === "ENOENT") {
        mlog("warn", "ralph", `Workspace gone — exiting loop`, { threadId, channelName, threadName });
        break;
      }
      throw err;
    }
    if (meta.state !== "running" && meta.state !== "sleeping") break;

    const stepNum = meta.stepCount + 1;

    try {
      meta.currentStep = stepNum;
      await writeRalphMeta(threadDir, meta);

      mlog("info", "ralph", `Step ${stepNum} starting`, {
        threadId,
        channelName,
        threadName,
      });

      await queue.send(threadId, `**— Step ${stepNum} —**`);

      const channelNotif = await queue.send(
        parentChannelId,
        `**${threadName}** — Step ${stepNum} running…`
      );

      const stepStart = Date.now();
      await executeStep(config, queue, threadId, channelName, threadName, stepNum);
      const stepDuration = Date.now() - stepStart;
      const durationStr = formatDuration(stepDuration);

      await queue.edit(
        parentChannelId,
        channelNotif.id,
        `**${threadName}** — Step ${stepNum} done (${durationStr})`
      );

      mlog("info", "ralph", `Step ${stepNum} completed`, {
        threadId,
        channelName,
        threadName,
        durationMs: stepDuration,
      });

      const updated = await readRalphMeta(threadDir);
      updated.stepCount = stepNum;
      updated.errorCount = 0;
      updated.lastStepAt = new Date().toISOString();
      updated.currentStep = undefined;

      if (updated.delayMinutes > 0) {
        const intervalMs = updated.delayMinutes * 60_000;
        const remaining = intervalMs - stepDuration;

        if (remaining > 0) {
          updated.state = "sleeping";
          await writeRalphMeta(threadDir, updated);
          mlog("debug", "ralph", `Sleeping ${formatDuration(remaining)} before next step`, {
            threadId,
            channelName,
            threadName,
          });
          await interruptibleSleep(remaining, signal);
          if (signal.aborted) break;
        } else {
          await queue.send(parentChannelId,
            `**${threadName}** — Step took ${durationStr}, proceeding immediately`
          );
        }

        updated.state = "running";
        await writeRalphMeta(threadDir, updated);
      } else {
        updated.state = "running";
        await writeRalphMeta(threadDir, updated);
      }
    } catch (err: any) {
      if (err?.code === "ENOENT") {
        mlog("warn", "ralph", `Workspace gone mid-step — exiting loop`, { threadId, channelName, threadName });
        break;
      }

      let updated: RalphMeta;
      try {
        updated = await readRalphMeta(threadDir);
      } catch (metaErr: any) {
        if (metaErr?.code === "ENOENT") {
          mlog("warn", "ralph", `Workspace gone during error handling — exiting loop`, { threadId, channelName, threadName });
          break;
        }
        throw metaErr;
      }

      updated.stepCount = stepNum;
      updated.currentStep = undefined;
      updated.errorCount++;
      const errMsg = err instanceof Error ? err.message : String(err);

      mlog("error", "ralph", `Step ${stepNum} error (${updated.errorCount}/3)`, {
        threadId,
        channelName,
        threadName,
        error: errMsg,
      });

      if (updated.errorCount >= 3) {
        updated.state = "error";
        await writeRalphMeta(threadDir, updated);
        mlog("error", "ralph", `Loop halted — 3 consecutive errors`, {
          threadId,
          channelName,
          threadName,
        });
        await queue.send(threadId, `**Loop stopped** — 3 consecutive errors. Use \`/resume\` to retry.`);
        await queue.send(parentChannelId, `**${threadName}** — Loop stopped (3 errors). Use \`/resume\` in thread.`);
        break;
      }

      await writeRalphMeta(threadDir, updated);
      await interruptibleSleep(5000, signal);
    }
  }

  const loop = activeLoops.get(threadId);
  if (loop) {
    try {
      const finalMeta = await readRalphMeta(threadDir);
      mlog("info", "ralph", `Loop ended`, {
        threadId,
        channelName,
        threadName,
        totalSteps: finalMeta.stepCount,
        runSince: loop.startedAt,
      });
    } catch {
      mlog("info", "ralph", `Loop ended (workspace removed)`, { threadId, channelName, threadName });
    }
  }

  activeLoops.delete(threadId);
}

function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  const secs = Math.round(ms / 1000);
  if (secs < 60) return `${secs}s`;
  const mins = Math.floor(secs / 60);
  const remainSecs = secs % 60;
  return remainSecs > 0 ? `${mins}m ${remainSecs}s` : `${mins}m`;
}

async function executeStep(
  config: Config,
  queue: DiscordSendQueue,
  threadId: string,
  channelName: string,
  threadName: string,
  stepNum: number
): Promise<void> {
  const ctx = await resolveContext(config, channelName, threadName);
  const prompt = await buildThreadPrompt(config, ctx);

  const stepDir = await ensureStepDir(ctx.threadDir!, stepNum);
  const startedAt = new Date().toISOString();
  await writeFile(join(stepDir, "PROMPT.md"), prompt);

  const rolloutPath = join(stepDir, "rollout.ndjson");
  const stream = await queue.createStream(threadId);

  const result = await runAgent(threadId, prompt, ctx.cwd, {
    rolloutPath,
    onChunk(chunk) { stream.push(chunk); },
  });

  await stream.finalize(result.output);

  const completedAt = new Date().toISOString();
  await writeFile(join(stepDir, "metadata.json"), JSON.stringify({
    step: stepNum,
    model: "claude-opus-4-6",
    exitCode: result.exitCode,
    durationMs: result.durationMs,
    promptLength: result.promptLength,
    startedAt,
    completedAt,
  }, null, 2));

  if (result.stderr) {
    await writeFile(join(stepDir, "errors.log"), result.stderr);
  }

  await appendChat(ctx.chatDir, {
    ts: completedAt,
    role: "assistant",
    content: result.output,
  });

  if (await shouldRegenSummary(ctx.chatDir)) {
    regenerateSummary(ctx.chatDir, config.openRouterKey);
  }
}

// ── Resume on Restart ───────────────────────────────────────────────────────

export async function resumeLoopsOnStartup(
  config: Config,
  queue: DiscordSendQueue,
  resolveThread: (channelName: string, threadName: string) => Promise<{ threadId: string; parentChannelId: string } | null>
): Promise<void> {
  try {
    const channels = await safeReaddir(config.workspaceRoot);
    let resumed = 0;
    let skipped = 0;
    for (const channelName of channels) {
      const threadsDir = join(config.workspaceRoot, channelName, "threads");
      const threads = await safeReaddir(threadsDir);
      for (const threadName of threads) {
        const threadDir = join(threadsDir, threadName);
        try {
          const meta = await readRalphMeta(threadDir);
          if (meta.state === "running" || meta.state === "sleeping") {
            const resolved = await resolveThread(channelName, threadName);

            if (meta.currentStep != null && meta.currentStep > meta.stepCount) {
              const killedStep = meta.currentStep;
              mlog("warn", "ralph", `Detected interrupted step on startup`, {
                channelName,
                threadName,
                killedStep,
                previousStepCount: meta.stepCount,
              });

              const stepDir = await ensureStepDir(threadDir, killedStep);
              await writeFile(join(stepDir, "metadata.json"), JSON.stringify({
                step: killedStep,
                model: "claude-opus-4-6",
                exitCode: null,
                durationMs: null,
                promptLength: null,
                startedAt: meta.lastStepAt || null,
                completedAt: null,
                status: "killed",
                reason: "process restart",
              }, null, 2));

              meta.stepCount = killedStep;
              meta.currentStep = undefined;
              meta.lastStepAt = new Date().toISOString();
              await writeRalphMeta(threadDir, meta);

              if (resolved) {
                await queue.send(resolved.threadId,
                  `**— Step ${killedStep} —**\n_Interrupted by process restart._`
                );
                await queue.send(resolved.parentChannelId,
                  `**${threadName}** — Step ${killedStep} interrupted (process restart)`
                );
              }
            }

            if (resolved) {
              mlog("info", "ralph", `Resuming loop after restart`, {
                channelName,
                threadName,
              });
              meta.state = "running";
              await writeRalphMeta(threadDir, meta);
              startLoop(config, queue, resolved.threadId, channelName, threadName, resolved.parentChannelId);
              resumed++;
            } else {
              mlog("warn", "ralph", `Could not resolve thread ID for resume`, {
                channelName,
                threadName,
              });
              skipped++;
            }
          }
        } catch {
          // skip invalid thread dirs
        }
      }
    }
    mlog("info", "ralph", `Startup resume complete`, { resumed, skipped });
  } catch {
    // workspace root may not exist yet
  }
}

// ── Utilities ───────────────────────────────────────────────────────────────

function interruptibleSleep(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    const timer = setTimeout(resolve, ms);
    const onAbort = () => {
      clearTimeout(timer);
      resolve();
    };
    signal.addEventListener("abort", onAbort, { once: true });
  });
}

async function safeReaddir(path: string): Promise<string[]> {
  try {
    return await readdir(path);
  } catch {
    return [];
  }
}

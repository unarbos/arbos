import { spawn, type ChildProcess } from "child_process";
import { createWriteStream, type WriteStream } from "fs";
import type { AgentResult, StreamCallback } from "./types.js";
import { mlog } from "./monitor.js";
import { getAllValues } from "./vault.js";

const running = new Map<string, ChildProcess>();

const AGENT_TIMEOUT_MS = 100 * 60 * 1000; // 100 minute hard timeout

export function isAgentRunning(contextKey: string): boolean {
  return running.has(contextKey);
}

export function killAgent(contextKey: string): boolean {
  const proc = running.get(contextKey);
  if (proc) {
    proc.kill("SIGTERM");
    running.delete(contextKey);
    mlog("warn", "agent", `Agent killed for context ${contextKey}`);
    return true;
  }
  return false;
}

export interface RunAgentOpts {
  rolloutPath?: string;
  onChunk?: StreamCallback;
}

export async function runAgent(
  contextKey: string,
  prompt: string,
  cwd: string,
  optsOrChunk?: RunAgentOpts | StreamCallback
): Promise<AgentResult> {
  const opts: RunAgentOpts = typeof optsOrChunk === "function"
    ? { onChunk: optsOrChunk }
    : optsOrChunk ?? {};
  const { rolloutPath, onChunk } = opts;

  if (running.has(contextKey)) {
    mlog("warn", "agent", `Rejected duplicate run for ${contextKey}`);
    return { output: "Agent is already running in this context.", stderr: "", exitCode: 1, durationMs: 0, promptLength: prompt.length };
  }

  const startTime = Date.now();
  const promptPreview = prompt.slice(0, 200).replace(/\n/g, "\\n");

  mlog("info", "agent", `Invocation started`, {
    contextKey,
    cwd,
    promptLength: prompt.length,
    promptPreview,
  });

  const vaultVars = await getAllValues().catch((err) => {
    mlog("warn", "agent", `Failed to load vault: ${err instanceof Error ? err.message : String(err)}`);
    return {} as Record<string, string>;
  });

  return new Promise<AgentResult>((resolve) => {
    const openRouterKey = vaultVars.OPENROUTER_KEY || process.env.OPENROUTER_KEY;
    if (!openRouterKey) throw new Error("OPENROUTER_KEY not found in vault or env");

    const orSettings = JSON.stringify({
      env: {
        ANTHROPIC_BASE_URL: "https://openrouter.ai/api",
        ANTHROPIC_API_KEY: openRouterKey,
      },
    });

    const proc = spawn(
      "claude",
      [
        "-p",
        "--verbose",
        "--model", "anthropic/claude-opus-4.6",
        "--dangerously-skip-permissions",
        "--output-format",
        "stream-json",
        "--setting-sources", "project",
        "--settings", orSettings,
      ],
      {
        cwd,
        stdio: ["pipe", "pipe", "pipe"],
        env: {
          ...process.env,
          ...vaultVars,
          ANTHROPIC_BASE_URL: "https://openrouter.ai/api",
          ANTHROPIC_API_KEY: openRouterKey,
        },
      }
    );

    proc.stdin!.end(prompt);

    running.set(contextKey, proc);

    let fullOutput = "";
    let stderrBuf = "";
    let lineBuffer = "";
    let settled = false;
    let rolloutStream: WriteStream | null = null;
    if (rolloutPath) {
      rolloutStream = createWriteStream(rolloutPath, { flags: "a" });
    }

    const timeout = setTimeout(() => {
      if (!settled) {
        mlog("error", "agent", `Timeout after ${AGENT_TIMEOUT_MS}ms — killing`, {
          contextKey,
          cwd,
          outputLength: fullOutput.length,
        });
        proc.kill("SIGKILL");
      }
    }, AGENT_TIMEOUT_MS);

    const settle = (result: AgentResult) => {
      if (settled) return;
      settled = true;
      clearTimeout(timeout);
      running.delete(contextKey);
      if (rolloutStream) rolloutStream.end();

      const duration = Date.now() - startTime;
      result.durationMs = duration;
      result.promptLength = prompt.length;
      result.stderr = stderrBuf;

      mlog("info", "agent", `Invocation completed`, {
        contextKey,
        exitCode: result.exitCode,
        durationMs: duration,
        outputLength: result.output.length,
        outputPreview: result.output.slice(0, 200).replace(/\n/g, "\\n"),
      });

      if (result.exitCode !== 0) {
        mlog("warn", "agent", `Non-zero exit`, {
          contextKey,
          exitCode: result.exitCode,
          stderr: stderrBuf.slice(0, 500),
        });
      }

      resolve(result);
    };

    proc.stdout!.on("data", (data: Buffer) => {
      if (rolloutStream) rolloutStream.write(data);
      lineBuffer += data.toString();
      const lines = lineBuffer.split("\n");
      lineBuffer = lines.pop() ?? "";

      for (const line of lines) {
        if (!line.trim()) continue;
        const text = extractText(line);
        if (text && text !== fullOutput) {
          const newContent = text.slice(fullOutput.length);
          fullOutput = text;
          if (newContent && onChunk) {
            onChunk(newContent, fullOutput);
          }
        }
      }
    });

    proc.stderr!.on("data", (data: Buffer) => {
      stderrBuf += data.toString();
    });

    proc.on("error", (err) => {
      mlog("error", "agent", `Process error: ${err.message}`, { contextKey });
      settle({
        output: `Agent process error: ${err.message}`,
        stderr: "",
        exitCode: 1,
        durationMs: 0,
        promptLength: prompt.length,
      });
    });

    proc.on("close", (code) => {
      if (lineBuffer.trim()) {
        const text = extractText(lineBuffer);
        if (text) fullOutput = text;
      }

      if (!fullOutput && stderrBuf) {
        fullOutput = `Error: ${stderrBuf.trim()}`;
      }

      settle({
        output: fullOutput || "_No output from agent._",
        stderr: "",
        exitCode: code ?? 1,
        durationMs: 0,
        promptLength: prompt.length,
      });
    });
  });
}

function extractText(line: string): string | null {
  try {
    const obj = JSON.parse(line);

    if (obj.type === "result" && typeof obj.result === "string") {
      return obj.result;
    }

    if (obj.type === "assistant" && obj.message?.content) {
      const content = obj.message.content;
      if (typeof content === "string") return content;
      if (Array.isArray(content)) {
        return content
          .filter((b: any) => b.type === "text")
          .map((b: any) => b.text)
          .join("");
      }
    }

    if (obj.type === "content_block_delta" && obj.delta?.text) {
      return obj.delta.text;
    }

    return null;
  } catch {
    return null;
  }
}

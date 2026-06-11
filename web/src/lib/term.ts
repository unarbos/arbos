/**
 * Terminals: the fourth tab kind, completing the rule that a tab holds a
 * typed *reference*, never content (chat → session, surface → path, run →
 * child session, terminal → job or shell id). Two flavors share the one tab:
 *
 *  - "job": an agent bash command. Every command already runs as a durable
 *    journaled job on disk, so the tab is a live tail of its journal plus
 *    status — read-only, kill-able, and exactly as durable as the job
 *    (it replays on resume because the job id rides the tool result's
 *    Details, the same channel surfaces use).
 *  - "shell": an interactive PTY the user opened, driven over a WebSocket.
 *    Live conversational state — it lives with the gateway process.
 */

import type { ToolResult } from "./types";

export type TermRef =
  | { kind: "job"; job: string; command?: string }
  | { kind: "shell"; id: string; cwd?: string };

export function termTitle(ref: TermRef): string {
  if (ref.kind === "job") {
    const line = ref.command?.split("\n")[0].trim();
    return line || ref.job;
  }
  return "Terminal";
}

/** Whether two references name the same terminal (tab dedupe). */
export function sameTerm(a: TermRef, b: TermRef): boolean {
  if (a.kind === "job" && b.kind === "job") return a.job === b.job;
  if (a.kind === "shell" && b.kind === "shell") return a.id === b.id;
  return false;
}

/** The job id a bash call recorded in its result's Details. */
export function detailsJob(result: ToolResult): string | undefined {
  const d = result.Details;
  if (typeof d === "object" && d !== null && "job" in d) {
    const job = (d as { job?: unknown }).job;
    if (typeof job === "string" && job) return job;
  }
  return undefined;
}

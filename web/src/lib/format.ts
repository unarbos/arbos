/** Small display formatters. */

import type { ToolCall } from "./types";

export function fmtElapsed(ms: number): string {
  const sec = Math.max(0, ms) / 1000;
  if (sec < 1) return `${Math.round(ms)}ms`;
  if (sec < 10) return `${sec.toFixed(1)}s`;
  if (sec < 60) return `${Math.round(sec)}s`;
  const m = Math.floor(sec / 60);
  const s = Math.round(sec % 60);
  return s ? `${m}m${s}s` : `${m}m`;
}

/** Compact single-line preview of a tool call's arguments. */
export function argsPreview(call: ToolCall, max = 100): string {
  const args = call.Args;
  if (args == null) return "";
  let s: string;
  if (typeof args === "string") {
    s = args;
  } else if (typeof args === "object") {
    s = Object.entries(args as Record<string, unknown>)
      .map(([k, v]) => `${k}=${typeof v === "string" ? v : JSON.stringify(v)}`)
      .join(" ");
  } else {
    s = JSON.stringify(args);
  }
  s = s.replace(/\s+/g, " ").trim();
  return s.length > max ? s.slice(0, max - 1) + "…" : s;
}

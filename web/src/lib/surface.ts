/**
 * Surfaces: the things the agent (or the user) can open in a panel beside
 * the chat. A surface is a typed *reference* — kind + path — never content:
 * the panel fetches the live file back through the gateway (/api/file,
 * /raw/…), so the event log stays small and an open panel is always current.
 *
 * The agent creates one with the `show` tool; the reference rides the tool
 * result's Details (the same channel delegate uses for childSession), which
 * makes live streaming and replay-on-resume work identically for free.
 */

import type { Theme } from "./themes";
import type { ToolResult } from "./types";

export type SurfaceKind =
  | "canvas"
  | "image"
  | "doc"
  | "pdf"
  | "code"
  | "sheet"
  | "prompt"
  | "dir"
  | "screencast"
  | "people";

export interface Surface {
  kind: SurfaceKind;
  /** Workspace-relative when under the root, else absolute. */
  path: string;
  /** Panel/tab label; falls back to the file name. */
  title?: string;
}

// The kinds the agent's show tool can present. "prompt" (the slash-command
// editor) is opened by the user from the / menu, never by a tool result.
const KINDS = new Set<string>(["canvas", "image", "doc", "pdf", "code", "sheet", "dir", "screencast"]);

const SHEET_EXT = new Set(["csv", "tsv", "tab"]);

const IMAGE_EXT = new Set(["png", "jpg", "jpeg", "gif", "webp", "svg", "ico", "bmp", "avif"]);

/**
 * The surface for a plain file reference (a clicked filename in the chat's
 * diff cards), kind inferred from the extension the way show's default
 * presentation would: markdown reads as a document, images render, PDFs open
 * in the browser's native viewer, anything else is code.
 */
export function fileSurface(path: string): Surface {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  const kind: SurfaceKind =
    ext === "md" || ext === "markdown"
      ? "doc"
      : ext === "pdf"
        ? "pdf"
        : IMAGE_EXT.has(ext)
          ? "image"
          : SHEET_EXT.has(ext)
            ? "sheet"
            : "code";
  return { kind, path };
}

/** The browser surface for a directory. "." is the workspace root. */
export function dirSurface(path: string, title?: string): Surface {
  return { kind: "dir", path, title };
}

/**
 * The People surface for a session: the human-to-human side chat for the
 * collaborators on a board, opened as its own tab beside the conversation
 * (not file-backed — its path is the session id, and it runs its own scoped
 * seam for presence + side-chat lines).
 */
export function peopleSurface(sessionId: string): Surface {
  return { kind: "people", path: sessionId, title: "People" };
}

/** A directory reference's child path ("." is the workspace root itself). */
export function joinDirPath(dir: string, name: string): string {
  if (dir === "." || dir === "") return name;
  if (dir === "/") return "/" + name;
  return dir + "/" + name;
}

/** The editor surface for one slash command's template file. A command not
 * yet on disk (the menu's create row) lands in the project prompts dir. */
export function promptSurface(name: string, path?: string): Surface {
  return {
    kind: "prompt",
    path: path || `.arbos/prompts/${name}.md`,
    title: `/${name}`,
  };
}

/** The surface a show tool recorded in its result's Details. */
export function detailsSurface(result: ToolResult): Surface | undefined {
  const d = result.Details;
  if (typeof d !== "object" || d === null || !("surface" in d)) return undefined;
  const s = (d as { surface?: unknown }).surface;
  if (typeof s !== "object" || s === null) return undefined;
  const { kind, path, title } = s as Record<string, unknown>;
  if (typeof path !== "string" || !path) return undefined;
  return {
    kind: typeof kind === "string" && KINDS.has(kind) ? (kind as SurfaceKind) : "code",
    path,
    title: typeof title === "string" && title ? title : undefined,
  };
}

/**
 * A UI-control command the agent issued (the `ui` tool): operate on the app's
 * own layout — open, close, or focus side panels. Rides the same tool-result
 * Details channel a surface does, executed on tool_finished.
 */
export type UIPanel =
  | "chat"
  | "terminal"
  | "browser"
  | "files"
  | "activity"
  | "history"
  | "settings";

export type UICommand =
  | {
      action: "close" | "focus";
      /** "all" or a substring matching a panel's title or file path. */
      target: string;
    }
  | {
      action: "open";
      panel: UIPanel;
      /** Starting directory for a terminal panel's shell. */
      cwd?: string;
    };

const UI_PANELS = new Set<string>([
  "chat",
  "terminal",
  "browser",
  "files",
  "activity",
  "history",
  "settings",
]);

/** The UI-control command a `ui` tool recorded in its result's Details. */
export function detailsUI(result: ToolResult): UICommand | undefined {
  const d = result.Details;
  if (typeof d !== "object" || d === null || !("ui" in d)) return undefined;
  const u = (d as { ui?: unknown }).ui;
  if (typeof u !== "object" || u === null) return undefined;
  const { action, target, panel, cwd } = u as Record<string, unknown>;
  if (action === "open") {
    if (typeof panel !== "string" || !UI_PANELS.has(panel)) return undefined;
    return {
      action,
      panel: panel as UIPanel,
      cwd: typeof cwd === "string" && cwd ? cwd : undefined,
    };
  }
  if ((action !== "close" && action !== "focus") || typeof target !== "string" || !target) {
    return undefined;
  }
  return { action, target };
}

/** URL serving the file raw (iframe src, img src, open-in-browser). */
export function rawUrl(path: string): string {
  return "/raw/" + path.split("/").map(encodeURIComponent).join("/");
}

export function surfaceTitle(s: Surface): string {
  if (s.title) return s.title;
  if (s.kind === "dir" && (s.path === "." || s.path === "")) return "Files";
  if (s.kind === "people") return "People";
  return s.path.split("/").pop() || s.path;
}

/* ------------------------------------------------------------------ */
/* Canvas theming: a canvas is written against the app's design tokens */
/* (`var(--color-*, fallback)`), never a palette of its own. The panel */
/* renders it via srcdoc with the ACTIVE theme's tokens injected, so a */
/* canvas repaints with the app — switch themes and it follows. Opened */
/* standalone (the /raw link), the fallbacks keep it presentable.      */
/* ------------------------------------------------------------------ */

const FONT_SANS =
  '-apple-system, BlinkMacSystemFont, "Segoe UI", Inter, Roboto, sans-serif';
const FONT_MONO =
  'ui-monospace, "SF Mono", "Cascadia Mono", "JetBrains Mono", Menlo, Consolas, monospace';
// Reading serif for long-form article surfaces (a blog's .article body). A
// canvas/blog reaches for var(--font-serif, …); inject it so it resolves.
const FONT_SERIF =
  'Newsreader, Charter, "Iowan Old Style", "Source Serif 4", "Source Serif Pro", Georgia, Cambria, "Times New Roman", serif';

/**
 * Wrap raw canvas HTML for the panel's iframe: a <base> into /raw so the
 * file's relative assets still resolve from srcdoc, and the active theme's
 * tokens appended LAST so they win over anything the file defines.
 */
export function themedCanvasDoc(html: string, theme: Theme, path: string): string {
  const dir = path.slice(0, path.lastIndexOf("/") + 1);
  const base = `<base href="${rawUrl(dir)}">`;
  const vars = Object.entries(theme.colors)
    .map(([token, value]) => `--color-${token}:${value};`)
    .join("");
  const style =
    `<style data-arbos-theme>:root{${vars}` +
    `--color-hover:color-mix(in srgb, var(--color-bright) 8%, transparent);` +
    `--font-sans:${FONT_SANS};--font-mono:${FONT_MONO};--font-serif:${FONT_SERIF};` +
    `color-scheme:${theme.dark ? "dark" : "light"};}</style>`;
  const headRe = /<head[^>]*>/i;
  const withBase = headRe.test(html)
    ? html.replace(headRe, (m) => m + base)
    : base + html;
  return withBase + style;
}

import { useCallback, useEffect, useRef, useState } from "react";
import { Check, Loader2 } from "lucide-react";

import { fetchFile, writeFile, HttpError } from "@/lib/api";
import { type Surface } from "@/lib/surface";
import { useDocumentVisible } from "@/lib/useDocumentVisible";

const STAT_POLL_MS = 2000;

/** Starter body for a command that doesn't exist on disk yet. */
const NEW_PROMPT_SEED = `---
description: What this command does (shown in the / menu)
argument-hint: <args>
---

Write the prompt here. $ARGUMENTS expands to everything typed after the
command; $1, $2… pick individual arguments.
`;

/** "/name" from the template's file path. */
function commandName(path: string): string {
  const base = path.split("/").pop() ?? path;
  return base.replace(/\.md$/, "");
}

type LoadState =
  | { phase: "loading" }
  | { phase: "error"; message: string }
  | { phase: "ready" };

/**
 * The slash-command editor: a "prompt" surface holding one template file
 * (.arbos/prompts/name.md), opened from the / menu's edit pencil or create
 * row. Plain text in, ⌘S/Save out — the engine reloads templates from disk
 * on every slash message, so a saved prompt runs as /name immediately, no
 * restart. A command not yet on disk opens seeded with a frontmatter
 * skeleton and saves into existence on first ⌘S.
 */
export function PromptEditor({
  surface,
  active,
}: {
  surface: Surface;
  active: boolean;
}) {
  const [state, setState] = useState<LoadState>({ phase: "loading" });
  const [content, setContent] = useState("");
  // The file's text as last seen on disk; null = not on disk yet (a create).
  const [saved, setSaved] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState("");
  const mtimeRef = useRef(0);
  const taRef = useRef<HTMLTextAreaElement>(null);

  const dirty = saved === null || content !== saved;
  const name = commandName(surface.path);

  useEffect(() => {
    let stale = false;
    fetchFile(surface.path)
      .then((info) => {
        if (stale) return;
        mtimeRef.current = info.mtime;
        setContent(info.content ?? "");
        setSaved(info.content ?? "");
        setState({ phase: "ready" });
      })
      .catch((e: unknown) => {
        if (stale) return;
        if (e instanceof HttpError && e.status === 404) {
          // The create flow: the command has no file yet — seed a skeleton.
          setContent(NEW_PROMPT_SEED);
          setSaved(null);
          setState({ phase: "ready" });
          return;
        }
        setState({
          phase: "error",
          message: e instanceof Error ? e.message : String(e),
        });
      });
    return () => {
      stale = true;
    };
  }, [surface.path]);

  // Pick up external edits (the agent rewriting an open prompt) while the
  // panel is clean; a dirty editor never clobbers the user's typing.
  const docVisible = useDocumentVisible();
  useEffect(() => {
    if (!active || !docVisible || state.phase !== "ready" || dirty) return;
    let stop = false;
    const check = () => {
      fetchFile(surface.path)
        .then((info) => {
          if (stop || info.mtime === mtimeRef.current) return;
          mtimeRef.current = info.mtime;
          setContent(info.content ?? "");
          setSaved(info.content ?? "");
        })
        .catch(() => {});
    };
    const id = window.setInterval(check, STAT_POLL_MS);
    return () => {
      stop = true;
      window.clearInterval(id);
    };
  }, [active, docVisible, state.phase, dirty, surface.path]);

  // The editor is something to type into (unlike the read-only surfaces):
  // opening it hands over the keyboard.
  useEffect(() => {
    if (state.phase === "ready" && active) taRef.current?.focus();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state.phase]);

  const save = useCallback(() => {
    if (saving) return;
    setSaving(true);
    setSaveError("");
    writeFile(surface.path, content)
      .then((info) => {
        mtimeRef.current = info.mtime;
        setSaved(content);
      })
      .catch((e: unknown) => {
        setSaveError(e instanceof Error ? e.message : String(e));
      })
      .finally(() => setSaving(false));
  }, [surface.path, content, saving]);

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if ((e.metaKey || e.ctrlKey) && e.key === "s") {
      e.preventDefault();
      if (dirty) save();
    }
  };

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      {state.phase === "loading" && (
        <div className="flex items-center gap-2 p-4 text-faint">
          <Loader2 size={13} className="animate-spin" /> Loading…
        </div>
      )}
      {state.phase === "error" && (
        <div className="p-4 text-[12.5px] text-red">{state.message}</div>
      )}
      {state.phase === "ready" && (
        <>
          <textarea
            ref={taRef}
            value={content}
            onChange={(e) => setContent(e.target.value)}
            onKeyDown={onKeyDown}
            spellCheck={false}
            className="min-h-0 w-full flex-1 resize-none bg-transparent px-4 py-3 font-mono text-[12px] leading-[1.6] text-text outline-none"
          />
          <div className="flex h-7 shrink-0 select-none items-center gap-2 border-t border-line/70 px-3 text-[11px] text-faint">
            {saveError ? (
              <span className="min-w-0 flex-1 truncate text-red">{saveError}</span>
            ) : (
              <span className="min-w-0 flex-1 truncate">
                Runs as <span className="font-mono text-muted">/{name}</span> — saved
                prompts are live on the next message.
              </span>
            )}
            {saved === null && (
              <span className="shrink-0 rounded border border-line px-1.5 py-px text-[10.5px] text-faint">
                new
              </span>
            )}
            <button
              type="button"
              onClick={save}
              disabled={!dirty || saving}
              title="Save (⌘S)"
              className={`flex h-[18px] shrink-0 cursor-pointer items-center gap-1.5 rounded px-2 text-[10.5px] transition-colors disabled:cursor-default ${
                dirty ? "bg-btn text-canvas hover:opacity-90" : "text-faint"
              }`}
            >
              {saving ? (
                <Loader2 size={10} className="animate-spin" />
              ) : (
                !dirty && <Check size={10} />
              )}
              {dirty ? "Save" : "Saved"}
            </button>
          </div>
        </>
      )}
    </div>
  );
}

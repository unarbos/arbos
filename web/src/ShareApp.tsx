import { useCallback, useEffect, useRef, useState } from "react";
import { Loader2, SendHorizontal } from "lucide-react";

import { Markdown } from "./components/Markdown";
import { fetchShareInfo, sendToShare, type SharePerm } from "./lib/api";

/**
 * The view a scoped chat link (/s/<token>) opens. A separate top-level app
 * from the live workspace — no tabs, no full seam — that renders the granted
 * session's transcript from /s/<token>/events and polls it so new turns
 * appear. A read grant is view-only; a write grant additionally shows a
 * composer that posts into the conversation via /s/<token>/send (a real turn).
 * A revoked or expired token answers 404, rendering the dead-link notice.
 */

/** One replayed transcript event — mirrors the gateway's replayJSON shape. */
interface ReplayEvent {
  type: "user" | "assistant" | "tool_result" | "interrupted";
  seq: number;
  text?: string;
  content?: string;
  call_id?: string;
  is_error?: boolean;
}

interface Replay {
  events: ReplayEvent[];
}

type Load =
  | { state: "loading" }
  | { state: "error" }
  | { state: "ready"; events: ReplayEvent[] };

const POLL_MS = 2500;

function tokenFromPath(): string {
  // /s/<token> or /s/<token>/...
  return window.location.pathname.split("/")[2] ?? "";
}

export function ShareApp() {
  const token = tokenFromPath();
  const [load, setLoad] = useState<Load>({ state: "loading" });
  const [perm, setPerm] = useState<SharePerm>("read");
  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);

  const refresh = useCallback(async () => {
    const r = await fetch(`/s/${encodeURIComponent(token)}/events`);
    if (!r.ok) throw new Error(String(r.status));
    const body = (await r.json()) as Replay;
    setLoad({ state: "ready", events: body.events ?? [] });
  }, [token]);

  // Initial load: permission (decides the composer) plus the transcript.
  useEffect(() => {
    let stop = false;
    fetchShareInfo(token)
      .then((info) => {
        if (!stop) setPerm(info.perm);
      })
      .catch(() => {});
    refresh().catch(() => {
      if (!stop) setLoad({ state: "error" });
    });
    return () => {
      stop = true;
    };
  }, [token, refresh]);

  // Poll so the recipient sees new turns (the owner's, or the response to a
  // message they just sent). Stops once the link goes dead.
  useEffect(() => {
    if (load.state === "error") return;
    const id = window.setInterval(() => {
      refresh().catch(() => {});
    }, POLL_MS);
    return () => window.clearInterval(id);
  }, [load.state, refresh]);

  const send = async () => {
    const text = draft.trim();
    if (!text || sending) return;
    setSending(true);
    try {
      await sendToShare(token, text);
      setDraft("");
      await refresh();
    } catch {
      // Surfaced by the next poll; keep the draft so nothing is lost.
    } finally {
      setSending(false);
    }
  };

  const canWrite = perm === "write" || perm === "admin";

  return (
    <div className="flex h-dvh flex-col bg-canvas text-text">
      <header className="flex shrink-0 items-center gap-2 border-b border-line bg-canvas/95 px-4 py-2.5">
        <span className="text-[13px] font-semibold text-bright">arbos</span>
        <span className="rounded-full border border-line px-2 py-0.5 text-[11px] text-muted">
          {canWrite ? "Shared conversation · you can reply" : "Shared conversation · read-only"}
        </span>
      </header>

      <div ref={scrollRef} className="min-h-0 flex-1 overflow-y-auto">
        <main className="mx-auto flex max-w-3xl flex-col gap-4 px-4 py-6">
          {load.state === "loading" && <div className="text-[13px] text-muted">Loading…</div>}
          {load.state === "error" && (
            <div className="text-[13px] text-muted">
              This link is no longer available — it may have expired or been revoked.
            </div>
          )}
          {load.state === "ready" &&
            (load.events.length === 0 ? (
              <div className="text-[13px] text-muted">This conversation is empty.</div>
            ) : (
              load.events.map((ev) => <TranscriptItem key={ev.seq} ev={ev} />)
            ))}
        </main>
      </div>

      {canWrite && load.state !== "error" && (
        <div className="shrink-0 border-t border-line bg-canvas px-4 py-3">
          <div className="mx-auto flex max-w-3xl items-end gap-2">
            <textarea
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.shiftKey) {
                  e.preventDefault();
                  void send();
                }
              }}
              rows={1}
              placeholder="Send a message…"
              className="min-h-[2.4rem] max-h-40 min-w-0 flex-1 resize-none rounded-lg border border-line bg-panel px-3 py-2 text-[13.5px] text-text outline-none focus:border-accent"
            />
            <button
              type="button"
              onClick={() => void send()}
              disabled={sending || !draft.trim()}
              className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-btn text-canvas transition-colors hover:bg-bright disabled:opacity-50"
            >
              {sending ? (
                <Loader2 size={16} className="animate-spin" />
              ) : (
                <SendHorizontal size={16} />
              )}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

function TranscriptItem({ ev }: { ev: ReplayEvent }) {
  switch (ev.type) {
    case "user":
      return (
        <div className="self-end rounded-2xl rounded-br-sm border border-line bg-panel px-3.5 py-2 text-[13.5px] text-text">
          <Markdown content={ev.text ?? ""} />
        </div>
      );
    case "assistant":
      return (
        <div className="text-[13.5px] leading-relaxed text-text">
          <Markdown content={ev.text ?? ""} />
        </div>
      );
    case "tool_result":
      return (
        <details className="rounded-md border border-line/70 bg-panel/50 text-[12px]">
          <summary className="cursor-pointer select-none px-3 py-1.5 text-muted">
            {ev.is_error ? "tool error" : "tool result"}
          </summary>
          <pre className="overflow-auto whitespace-pre-wrap px-3 pb-2 font-mono text-[11.5px] text-faint">
            {ev.content ?? ""}
          </pre>
        </details>
      );
    case "interrupted":
      return (
        <div className="flex items-center gap-2 text-[11px] text-faint">
          <span className="h-px flex-1 bg-line" />
          interrupted
          <span className="h-px flex-1 bg-line" />
        </div>
      );
    default: {
      const _exhaustive: never = ev.type;
      return _exhaustive;
    }
  }
}

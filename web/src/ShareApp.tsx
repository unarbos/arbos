import { useEffect, useState } from "react";

import { Markdown } from "./components/Markdown";

/**
 * The read-only view a scoped chat link (/s/<token>) opens. It is a separate
 * top-level app from the live workspace: no composer, no websocket seam, no
 * tabs — just the granted session's transcript, fetched once from
 * /s/<token>/events (the same replay projection the authed history endpoint
 * serves). A revoked or expired token answers 404, which renders as the
 * dead-link notice rather than a broken page.
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

function tokenFromPath(): string {
  // /s/<token> or /s/<token>/...
  return window.location.pathname.split("/")[2] ?? "";
}

export function ShareApp() {
  const [load, setLoad] = useState<Load>({ state: "loading" });

  useEffect(() => {
    const token = tokenFromPath();
    let stop = false;
    fetch(`/s/${encodeURIComponent(token)}/events`)
      .then((r) => {
        if (!r.ok) throw new Error(String(r.status));
        return r.json() as Promise<Replay>;
      })
      .then((body) => {
        if (!stop) setLoad({ state: "ready", events: body.events ?? [] });
      })
      .catch(() => {
        if (!stop) setLoad({ state: "error" });
      });
    return () => {
      stop = true;
    };
  }, []);

  return (
    <div className="min-h-dvh bg-canvas text-text">
      <header className="sticky top-0 z-10 flex items-center gap-2 border-b border-line bg-canvas/95 px-4 py-2.5 backdrop-blur">
        <span className="text-[13px] font-semibold text-bright">arbos</span>
        <span className="rounded-full border border-line px-2 py-0.5 text-[11px] text-muted">
          Shared conversation · read-only
        </span>
      </header>
      <main className="mx-auto flex max-w-3xl flex-col gap-4 px-4 py-6">
        {load.state === "loading" && (
          <div className="text-[13px] text-muted">Loading…</div>
        )}
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

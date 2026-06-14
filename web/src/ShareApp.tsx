import { useCallback, useEffect, useRef, useState } from "react";
import { Loader2, SendHorizontal } from "lucide-react";

import { ChatView } from "./components/ChatView";
import { fetchShareInfo, sendToShare, type ReplayEvent, type SharePerm } from "./lib/api";
import { replayToItems, type TranscriptItem } from "./lib/transcript";

/**
 * The view a scoped chat link (/s/<token>) opens. It renders the granted
 * session with the SAME transcript component the live app uses (ChatView via
 * replayToItems) — no second chat renderer to drift — minus the controls a
 * guest shouldn't have (model picker, history, tabs). A read grant is
 * view-only; a write grant adds a composer that posts into the conversation
 * via /s/<token>/send (a real turn). The transcript is polled so new turns
 * appear. A revoked/expired token answers 404 → the dead-link notice.
 */

interface Replay {
  events: ReplayEvent[];
}

type Load =
  | { state: "loading" }
  | { state: "error" }
  | { state: "ready"; items: TranscriptItem[] };

const POLL_MS = 2500;

function tokenFromPath(): string {
  return window.location.pathname.split("/")[2] ?? "";
}

export function ShareApp() {
  const token = tokenFromPath();
  const [load, setLoad] = useState<Load>({ state: "loading" });
  const [perm, setPerm] = useState<SharePerm>("read");
  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);
  // True between sending a message and the agent's reply landing, so the
  // transcript shows the same "working" heartbeat the live app does.
  const [awaiting, setAwaiting] = useState(false);
  const countRef = useRef(0);

  const refresh = useCallback(async () => {
    const r = await fetch(`/s/${encodeURIComponent(token)}/events`);
    if (!r.ok) throw new Error(String(r.status));
    const body = (await r.json()) as Replay;
    const items = replayToItems(body.events ?? []);
    if (items.length > countRef.current) setAwaiting(false);
    countRef.current = items.length;
    setLoad({ state: "ready", items });
  }, [token]);

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
    setAwaiting(true);
    try {
      await sendToShare(token, text);
      setDraft("");
      await refresh();
    } catch {
      setAwaiting(false);
    } finally {
      setSending(false);
    }
  };

  const canWrite = perm === "write" || perm === "admin";

  return (
    <div className="flex h-dvh flex-col bg-canvas text-text">
      <header className="flex shrink-0 items-center gap-2 border-b border-line px-4 py-2.5">
        <span className="text-[13px] font-semibold text-bright">arbos</span>
        <span className="rounded-full border border-line px-2 py-0.5 text-[11px] text-muted">
          {canWrite ? "Shared conversation · you can reply" : "Shared conversation · read-only"}
        </span>
      </header>

      {load.state === "loading" && (
        <div className="flex-1 px-4 py-6 text-[13px] text-muted">Loading…</div>
      )}
      {load.state === "error" && (
        <div className="flex-1 px-4 py-6 text-[13px] text-muted">
          This link is no longer available — it may have expired or been revoked.
        </div>
      )}
      {load.state === "ready" && <ChatView items={load.items} working={awaiting} />}

      {canWrite && load.state !== "error" && (
        <div className="shrink-0 border-t border-line px-3.5 py-3">
          <div className="mx-auto flex max-w-4xl items-end gap-2">
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
              className="min-h-[2.5rem] max-h-40 min-w-0 flex-1 resize-none rounded-lg border border-line bg-panel px-3 py-2 text-[13.5px] text-text outline-none focus:border-accent"
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

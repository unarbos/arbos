import { useEffect, useRef, useState } from "react";
import { Loader2, Orbit } from "lucide-react";

import { TranscriptList } from "./ChatView";
import { fetchReplay } from "@/lib/api";
import { subscribeActivity } from "@/lib/activity";
import { replayToItems, type TranscriptItem } from "@/lib/transcript";
import { useDocumentVisible } from "@/lib/useDocumentVisible";

const REPLAY_POLL_MS = 2000;

/** A machine-spawned run (scheduled firing or delegated sub-agent) opened in
 *  its own tab: a typed reference, never content — the view fetches live. */
export interface RunRef {
  session: string;
  label: string;
}

/**
 * A run tab's body: the sub-agent's transcript, read-only — the parent owns
 * the conversation; this is a window onto the worker. The tab is independent
 * of the chat that spawned it, so a long-running run stays watchable while
 * the user keeps chatting (or closes the parent entirely): the transcript
 * re-fetches while the run is live, and the run's status is polled for as
 * long as the tab exists so the strip's spinner stays honest.
 */
export function RunView({
  run,
  active,
  onBusy,
}: {
  run: RunRef;
  /** Visible in its pane — transcript polling pauses while hidden. */
  active: boolean;
  /** Live status for the tab strip's spinner. */
  onBusy?: (busy: boolean) => void;
}) {
  const [items, setItems] = useState<TranscriptItem[] | null>(null);
  const [running, setRunning] = useState(true);
  const scrollRef = useRef<HTMLDivElement>(null);
  const pinnedRef = useRef(true);
  const onBusyRef = useRef(onBusy);
  onBusyRef.current = onBusy;

  // Status: is the run still live? Rides the shared activity poller (one
  // /api/activity interval per window, even with many run tabs open) so the
  // spinner on the tab itself stays truthful while the tab is hidden. A run
  // that has aged out of the activity window is finished.
  useEffect(
    () =>
      subscribeActivity((a) => {
        const me = a.runs.find((r) => r.id === run.session);
        setRunning(me?.active ?? false);
      }),
    [run.session],
  );

  useEffect(() => {
    onBusyRef.current?.(running);
  }, [running]);

  // Transcript: fetched when the tab shows, re-fetched while the run is live.
  // When `running` flips false this effect re-runs once more — the final load
  // catches the run's last events, then the interval stops. Pauses with the
  // window hidden; the visibility flip re-runs the effect, refreshing at once.
  const docVisible = useDocumentVisible();
  useEffect(() => {
    if (!active || !docVisible) return;
    let stop = false;
    const load = () => {
      fetchReplay(run.session)
        .then((events) => {
          if (!stop) setItems(replayToItems(events));
        })
        .catch(() => {});
    };
    load();
    if (!running) return () => { stop = true; };
    const id = window.setInterval(load, REPLAY_POLL_MS);
    return () => {
      stop = true;
      window.clearInterval(id);
    };
  }, [active, docVisible, running, run.session]);

  // Follow the tail while pinned to the bottom (a scroll up unpins).
  useEffect(() => {
    const el = scrollRef.current;
    if (el && pinnedRef.current) el.scrollTop = el.scrollHeight;
  }, [items]);

  const onScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    pinnedRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
  };

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <div className="flex h-9 shrink-0 select-none items-center gap-2 border-b border-line/70 px-3">
        <Orbit size={13} className="shrink-0 text-muted" />
        <span className="min-w-0 truncate text-[12.5px] text-bright">
          {run.label}
        </span>
        <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-faint">
          {run.session}
        </span>
        {running && (
          <Loader2 size={13} className="shrink-0 animate-spin text-faint" />
        )}
      </div>
      <div
        ref={scrollRef}
        onScroll={onScroll}
        className="min-h-0 flex-1 overflow-y-auto"
      >
        {items === null || items.length === 0 ? (
          <div className="mx-auto w-full max-w-4xl px-4 py-6 text-faint">
            {items === null
              ? "Loading…"
              : running
                ? "Waiting for the sub-agent's first events…"
                : "No transcript."}
          </div>
        ) : (
          <TranscriptList items={items} working={running} />
        )}
      </div>
    </div>
  );
}

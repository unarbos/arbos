import { ActivityView } from "./ActivityView";
import { HistoryView } from "./HistoryView";
import type { SessionSummary } from "@/lib/api";

/**
 * The activity + history panel, merged: past sessions on the left, the
 * agent's standing obligations and live runs on the right, side by side in
 * one tab. Each half keeps its own poller and scroll, so the columns stay
 * independent — searching history never disturbs the running-agents list.
 */
export function ActivityHistoryView({
  active,
  onOpenSession,
  onOpenChat,
  onOpenPlan,
  onBusy,
}: {
  /** Visible in its pane — each half's polling pauses while hidden. */
  active: boolean;
  /** Open (or focus) a past session's chat. */
  onOpenSession: (s: SessionSummary) => void;
  /** Open the chat that owns a run / obligation. */
  onOpenChat: (chat: string) => void;
  /** Open the plan detail view (goal tree + code) for a node. */
  onOpenPlan?: (node: number) => void;
  /** Any run live right now — drives the tab strip's spinner. */
  onBusy?: (busy: boolean) => void;
}) {
  return (
    <div className="flex min-h-0 min-w-0 flex-1">
      <Column label="Sessions">
        <HistoryView active={active} onOpenSession={onOpenSession} />
      </Column>
      <Column label="Running agents" className="border-l border-line">
        <ActivityView
          onOpenChat={onOpenChat}
          onOpenPlan={onOpenPlan}
          onBusy={onBusy}
        />
      </Column>
    </div>
  );
}

function Column({
  label,
  className,
  children,
}: {
  label: string;
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <div className={`flex min-h-0 min-w-0 flex-1 flex-col ${className ?? ""}`}>
      <div className="shrink-0 border-b border-line/60 bg-bar px-4 py-2 text-[11px] font-semibold uppercase tracking-wider text-faint select-none">
        {label}
      </div>
      {children}
    </div>
  );
}

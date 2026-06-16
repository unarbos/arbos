import { useEffect, useRef, useState } from "react";
import { Loader2, MessageSquare, Orbit, Repeat, X } from "lucide-react";

import {
  cancelPlanNode,
  stopRun,
  type Activity,
  type ActivityRun,
  type StandingTask,
} from "@/lib/api";
import { subscribeActivity } from "@/lib/activity";

function age(ms: number): string {
  const sec = Math.max(0, (Date.now() - ms) / 1000);
  if (sec < 60) return `${Math.round(sec)}s ago`;
  if (sec < 3600) return `${Math.round(sec / 60)}m ago`;
  if (sec < 86400) return `${Math.round(sec / 3600)}h ago`;
  return `${Math.round(sec / 86400)}d ago`;
}

/**
 * The whole-organism view as a tab: every standing obligation in the global
 * plan plus the agent's recent autonomous runs, across all chats. Each row
 * links into its owning conversation. Activity rides the shared poller (one
 * /api/activity interval per window, paused while the window is hidden), so
 * the strip's spinner stays live even when this tab is behind another —
 * without every consumer running its own identical poll.
 */
export function ActivityView({
  onOpenChat,
  onOpenPlan,
  onBusy,
  readOnly,
}: {
  onOpenChat: (chat: string) => void;
  /** Open the plan detail view (goal tree + code) for a node. */
  onOpenPlan?: (node: number) => void;
  /** Any run live right now — drives the tab strip's spinner. */
  onBusy?: (busy: boolean) => void;
  /** Observe-only (a share guest): the cancel/stop controls are hidden — the
   *  mutation routes are 403 for a guest anyway, so the affordances don't even
   *  appear. */
  readOnly?: boolean;
}) {
  const [activity, setActivity] = useState<Activity | null>(null);
  const onBusyRef = useRef(onBusy);
  onBusyRef.current = onBusy;

  useEffect(() => subscribeActivity(setActivity), []);

  useEffect(() => {
    onBusyRef.current?.(
      activity?.runs.some((r) => r.active && !r.stale) ?? false,
    );
  }, [activity]);

  const cancel = (node: number) => {
    cancelPlanNode(node)
      .then(() =>
        setActivity((a) =>
          a ? { ...a, standing: a.standing.filter((t) => t.node !== node) } : a,
        ),
      )
      .catch(() => {});
  };

  const stop = (run: ActivityRun) => {
    stopRun(run.id).catch(() => {});
    // A run from a recurring node keeps re-firing unless the schedule is
    // cancelled too — offer that, then drop the now-cancelled obligation.
    if (
      run.node &&
      window.confirm(
        `Also cancel the recurring schedule (node #${run.node}) so no new runs spawn?`,
      )
    ) {
      cancelPlanNode(run.node)
        .then(() =>
          setActivity((a) =>
            a
              ? { ...a, standing: a.standing.filter((t) => t.node !== run.node) }
              : a,
          ),
        )
        .catch(() => {});
    }
  };

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-2xl px-4 py-4">
          {activity === null ? (
            <div className="text-faint">Loading…</div>
          ) : (
            <>
              <Section label="Standing obligations">
                {activity.standing.length === 0 && (
                  <div className="text-[12px] text-faint">None</div>
                )}
                {activity.standing.map((t) => (
                  <StandingRow
                    key={t.node}
                    task={t}
                    onOpen={onOpenPlan ? () => onOpenPlan(t.node) : undefined}
                    onOpenChat={t.chat ? () => onOpenChat(t.chat!) : undefined}
                    onCancel={readOnly ? undefined : () => cancel(t.node)}
                  />
                ))}
              </Section>
              <Section label="Recent runs">
                {activity.runs.length === 0 && (
                  <div className="text-[12px] text-faint">None</div>
                )}
                {activity.runs.map((r) => (
                  <RunRow
                    key={r.id}
                    run={r}
                    onOpen={() => onOpenChat(r.chat)}
                    onStop={readOnly ? undefined : () => stop(r)}
                  />
                ))}
              </Section>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

function Section({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="mb-4">
      <div className="mb-1.5 text-[11px] uppercase tracking-wider text-faint select-none">
        {label}
      </div>
      <div className="space-y-1">{children}</div>
    </div>
  );
}

function StandingRow({
  task,
  onOpen,
  onOpenChat,
  onCancel,
}: {
  task: StandingTask;
  /** Open the plan detail view (goal tree + code). */
  onOpen?: () => void;
  /** Open the owning conversation. */
  onOpenChat?: () => void;
  /** Cancel the obligation; omitted in observe-only (share) mode. */
  onCancel?: () => void;
}) {
  return (
    <div className="group flex min-w-0 items-start gap-2 rounded-md border border-line/60 px-2.5 py-1.5">
      <Repeat size={12} className="mt-1 shrink-0 text-muted" />
      <button
        type="button"
        onClick={onOpen}
        disabled={!onOpen}
        title={onOpen ? "View plan & code" : undefined}
        className={`min-w-0 flex-1 text-left ${onOpen ? "cursor-pointer" : "cursor-default"}`}
      >
        <div className="break-words text-[12.5px] text-text">
          {task.goal}
          {task.when && <span className="text-faint"> · {task.when}</span>}
        </div>
        {task.outcome && (
          <div className="truncate text-[11.5px] text-faint">{task.outcome}</div>
        )}
      </button>
      {onOpenChat && (
        <button
          type="button"
          onClick={onOpenChat}
          title="Open owning chat"
          className="mt-0.5 flex size-5 shrink-0 cursor-pointer items-center justify-center rounded text-faint opacity-0 transition-all group-hover:opacity-100 hover:bg-hover hover:text-text"
        >
          <MessageSquare size={11} />
        </button>
      )}
      {onCancel && (
        <button
          type="button"
          onClick={onCancel}
          title={`Cancel #${task.node}`}
          className="mt-0.5 flex size-5 shrink-0 cursor-pointer items-center justify-center rounded text-faint opacity-0 transition-all group-hover:opacity-100 hover:bg-hover hover:text-red"
        >
          <X size={11} />
        </button>
      )}
    </div>
  );
}

function RunRow({
  run,
  onOpen,
  onStop,
}: {
  run: ActivityRun;
  onOpen: () => void;
  /** Stop the run / cancel its schedule; omitted in observe-only (share) mode. */
  onStop?: () => void;
}) {
  const label =
    run.kind === "scheduled"
      ? run.node
        ? `Scheduled run · node #${run.node}`
        : "Scheduled run"
      : "Delegated run";
  // A stale run's task is cancelled or finished — it will never fire again, so
  // dim it and say so rather than letting it read as a live recurrence. The
  // label says "ended", not "cancelled": staleness only means the plan node is
  // no longer live, and a run that completed cleanly lands here too. Pure
  // history (stale and idle) has nothing to stop; a still-live run does.
  const stoppable = run.active || !run.stale;
  return (
    <div className="group flex min-w-0 items-center gap-2 rounded-md px-1.5 py-1">
      <button
        type="button"
        onClick={onOpen}
        title={run.stale ? "Task no longer active · open owning chat" : "Open owning chat"}
        className={`flex min-w-0 flex-1 cursor-pointer items-center gap-2 text-left transition-colors hover:text-text ${run.stale ? "opacity-55" : ""}`}
      >
        <Orbit
          size={12}
          className={`shrink-0 ${run.stale ? "text-faint" : "text-muted"}`}
        />
        <span className="min-w-0 flex-1 truncate text-[12.5px] text-muted">
          {label} <span className="text-faint">· {age(run.updated_at)}</span>
          {run.stale && <span className="text-faint"> · ended</span>}
        </span>
        {run.active && !run.stale && (
          <Loader2 size={11} className="shrink-0 animate-spin text-faint" />
        )}
      </button>
      {stoppable && onStop && (
        <button
          type="button"
          onClick={onStop}
          title={run.active ? "Stop run" : "Stop / cancel schedule"}
          className="flex size-5 shrink-0 cursor-pointer items-center justify-center rounded text-faint opacity-0 transition-all group-hover:opacity-100 hover:bg-hover hover:text-red"
        >
          <X size={11} />
        </button>
      )}
    </div>
  );
}

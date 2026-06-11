import { useEffect, useState } from "react";
import { Activity as ActivityIcon, Loader2, Orbit, Repeat, X } from "lucide-react";

import {
  cancelPlanNode,
  fetchActivity,
  stopRun,
  type Activity,
  type ActivityRun,
  type StandingTask,
} from "@/lib/api";

const ACTIVITY_POLL_MS = 5000;

function age(ms: number): string {
  const sec = Math.max(0, (Date.now() - ms) / 1000);
  if (sec < 60) return `${Math.round(sec)}s ago`;
  if (sec < 3600) return `${Math.round(sec / 60)}m ago`;
  if (sec < 86400) return `${Math.round(sec / 3600)}h ago`;
  return `${Math.round(sec / 86400)}d ago`;
}

/**
 * The whole-organism view: every standing obligation in the global plan plus
 * the agent's recent autonomous runs, across all chats. Each row links into
 * its owning conversation — scoped chats for focus, one place to see
 * everything the agent is carrying.
 */
export function ActivityPanel({
  onOpenChat,
  onClose,
}: {
  onOpenChat: (chat: string) => void;
  onClose: () => void;
}) {
  const [activity, setActivity] = useState<Activity | null>(null);

  useEffect(() => {
    let stop = false;
    const tick = () => {
      fetchActivity()
        .then((a) => {
          if (!stop) setActivity(a);
        })
        .catch(() => {});
    };
    tick();
    const id = window.setInterval(tick, ACTIVITY_POLL_MS);
    return () => {
      stop = true;
      window.clearInterval(id);
    };
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

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
    <div className="fixed inset-y-0 right-0 z-30 flex w-[min(440px,92vw)] flex-col border-l border-line bg-canvas shadow-[-24px_0_48px_rgba(0,0,0,0.35)]">
      <div className="flex h-10 shrink-0 items-center gap-2 border-b border-line/70 px-3.5 select-none">
        <ActivityIcon size={14} className="shrink-0 text-muted" />
        <span className="min-w-0 flex-1 truncate text-[12.5px] text-bright">
          Agent activity
        </span>
        <button
          type="button"
          onClick={onClose}
          title="Close (esc)"
          className="flex size-6 shrink-0 cursor-pointer items-center justify-center rounded text-muted transition-colors hover:bg-card hover:text-text"
        >
          <X size={14} />
        </button>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-3.5 py-3">
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
                  onOpen={t.chat ? () => onOpenChat(t.chat!) : undefined}
                  onCancel={() => cancel(t.node)}
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
                  onStop={() => stop(r)}
                />
              ))}
            </Section>
          </>
        )}
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
  onCancel,
}: {
  task: StandingTask;
  onOpen?: () => void;
  onCancel: () => void;
}) {
  return (
    <div className="group flex min-w-0 items-start gap-2 rounded-md border border-line/60 px-2.5 py-1.5">
      <Repeat size={12} className="mt-1 shrink-0 text-muted" />
      <button
        type="button"
        onClick={onOpen}
        disabled={!onOpen}
        title={onOpen ? "Open owning chat" : undefined}
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
      <button
        type="button"
        onClick={onCancel}
        title={`Cancel #${task.node}`}
        className="mt-0.5 flex size-5 shrink-0 cursor-pointer items-center justify-center rounded text-faint opacity-0 transition-all group-hover:opacity-100 hover:bg-hover hover:text-red"
      >
        <X size={11} />
      </button>
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
  onStop: () => void;
}) {
  const label =
    run.kind === "scheduled"
      ? run.node
        ? `Scheduled run · node #${run.node}`
        : "Scheduled run"
      : "Delegated run";
  // A stale run's task is cancelled or finished — it will never fire again, so
  // dim it and say so rather than letting it read as a live recurrence. Pure
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
          {run.stale && <span className="text-faint"> · cancelled</span>}
        </span>
        {run.active && !run.stale && (
          <Loader2 size={11} className="shrink-0 animate-spin text-faint" />
        )}
      </button>
      {stoppable && (
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

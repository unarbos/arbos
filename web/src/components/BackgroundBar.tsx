import { useState } from "react";
import {
  ChevronRight,
  Clock,
  Loader2,
  Orbit,
  SquareTerminal,
  X,
} from "lucide-react";

import type { ChildRun } from "@/lib/api";
import type { BackgroundJob, ScheduledTask } from "@/lib/transcript";

const RUNS_SHOWN = 5;

function runAge(ms: number): string {
  const sec = Math.max(0, (Date.now() - ms) / 1000);
  if (sec < 60) return `${Math.round(sec)}s ago`;
  if (sec < 3600) return `${Math.round(sec / 60)}m ago`;
  return `${Math.round(sec / 3600)}h ago`;
}

/**
 * Live background work, pinned above the composer like Cursor's
 * "1 background terminal" rows: collapsed counts that expand into the
 * running commands, the plan's armed clocks/callbacks, and this chat's
 * scheduled agent runs — each run an openable sub-agent window.
 */
export function BackgroundBar({
  jobs,
  scheduled,
  runs,
  onOpenRun,
  onOpenJob,
  onKillJob,
  onCancelTask,
  onStopRun,
}: {
  jobs: BackgroundJob[];
  scheduled: ScheduledTask[];
  runs: ChildRun[];
  onOpenRun: (r: ChildRun) => void;
  /** Open a background terminal as a tab tailing its live journal. */
  onOpenJob: (j: BackgroundJob) => void;
  onKillJob: (id: string) => void;
  onCancelTask: (id: number) => void;
  onStopRun: (r: ChildRun) => void;
}) {
  const [open, setOpen] = useState(false);
  const activeRuns = runs.filter((r) => r.active).length;

  const parts: string[] = [];
  if (jobs.length > 0) {
    parts.push(`${jobs.length} background terminal${jobs.length > 1 ? "s" : ""}`);
  }
  if (scheduled.length > 0) {
    parts.push(`${scheduled.length} scheduled task${scheduled.length > 1 ? "s" : ""}`);
  }
  if (runs.length > 0) {
    parts.push(
      activeRuns > 0
        ? `${activeRuns} agent run${activeRuns > 1 ? "s" : ""} live`
        : `${runs.length} agent run${runs.length > 1 ? "s" : ""}`,
    );
  }

  return (
    <div className="select-none">
      <button
        type="button"
        onClick={() => setOpen(!open)}
        className="flex w-full cursor-pointer items-center gap-1 rounded-md px-1 py-0.5 text-[12px] text-muted transition-colors hover:text-text"
      >
        <ChevronRight
          size={12}
          className={`shrink-0 text-faint transition-transform ${open ? "rotate-90" : ""}`}
        />
        {parts.join(" · ")}
        {!open && activeRuns > 0 && (
          <Loader2 size={11} className="ml-1 shrink-0 animate-spin text-faint" />
        )}
      </button>
      {open && (
        <div className="space-y-1 py-1 pl-5">
          {jobs.map((j) => (
            <div
              key={j.id}
              className="group flex min-w-0 items-center gap-2 text-[12px] text-muted"
            >
              <button
                type="button"
                onClick={() => onOpenJob(j)}
                title={`Open ${j.id} as a terminal tab`}
                className="flex min-w-0 flex-1 cursor-pointer items-center gap-2 rounded text-left transition-colors hover:text-text"
              >
                <SquareTerminal size={12} className="shrink-0 text-faint" />
                <span className="min-w-0 flex-1 truncate font-mono text-[11.5px]">
                  {j.command || j.id}
                </span>
                <ChevronRight size={12} className="shrink-0 text-faint" />
              </button>
              <button
                type="button"
                onClick={() => onKillJob(j.id)}
                title={`Kill ${j.id}`}
                className="flex size-5 shrink-0 cursor-pointer items-center justify-center rounded text-faint opacity-0 transition-all group-hover:opacity-100 hover:bg-hover hover:text-red"
              >
                <X size={11} />
              </button>
            </div>
          ))}
          {scheduled.map((t) => {
            // A scheduled node fires agent runs that carry its node id; the
            // latest one is the work — the run whose transcript holds the code
            // the firing wrote. Surface it from the expanded row so the plan is
            // a door to its output, not just a clock with a cancel button.
            const latestRun = runs
              .filter((r) => r.node === t.id)
              .sort((a, b) => b.updated_at - a.updated_at)[0];
            return (
              <ScheduledRow
                key={t.id}
                task={t}
                latestRun={latestRun}
                onOpenRun={onOpenRun}
                onCancelTask={onCancelTask}
              />
            );
          })}
          {runs.slice(0, RUNS_SHOWN).map((r) => (
            <div
              key={r.id}
              className="group flex min-w-0 items-center gap-2 text-[12px] text-muted"
            >
              <button
                type="button"
                onClick={() => onOpenRun(r)}
                className="flex min-w-0 flex-1 cursor-pointer items-center gap-2 rounded text-left transition-colors hover:text-text"
              >
                <Orbit size={12} className="shrink-0 text-faint" />
                <span className="min-w-0 truncate">
                  {r.node ? `Run · node #${r.node}` : "Run"}{" "}
                  <span className="text-faint">· {runAge(r.updated_at)}</span>
                </span>
                {r.active && (
                  <Loader2 size={11} className="shrink-0 animate-spin text-faint" />
                )}
                <ChevronRight size={12} className="shrink-0 text-faint" />
              </button>
              <button
                type="button"
                onClick={() => onStopRun(r)}
                title={r.active ? "Stop run" : "Stop / cancel schedule"}
                className="flex size-5 shrink-0 cursor-pointer items-center justify-center rounded text-faint opacity-0 transition-all group-hover:opacity-100 hover:bg-hover hover:text-red"
              >
                <X size={11} />
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * One scheduled plan node. Click the row to expand it: the truncated goal
 * unfolds into its full text and trigger, and (once it has fired) a door to
 * the latest agent run it spawned.
 */
function ScheduledRow({
  task,
  latestRun,
  onOpenRun,
  onCancelTask,
}: {
  task: ScheduledTask;
  latestRun: ChildRun | undefined;
  onOpenRun: (r: ChildRun) => void;
  onCancelTask: (id: number) => void;
}) {
  const [open, setOpen] = useState(false);

  return (
    <div className="group min-w-0 text-[12px] text-muted">
      <div className="flex min-w-0 items-center gap-2">
        <button
          type="button"
          onClick={() => setOpen((v) => !v)}
          aria-expanded={open}
          title={open ? "Collapse" : "Expand"}
          className="flex min-w-0 flex-1 cursor-pointer items-center gap-2 rounded text-left transition-colors hover:text-text"
        >
          <ChevronRight
            size={12}
            className={`shrink-0 text-faint transition-transform ${open ? "rotate-90" : ""}`}
          />
          <Clock size={12} className="shrink-0 text-faint" />
          <span className="min-w-0 flex-1 truncate">
            {task.goal} <span className="text-faint">· {task.when}</span>
          </span>
          {latestRun?.active && (
            <Loader2 size={11} className="shrink-0 animate-spin text-faint" />
          )}
        </button>
        <button
          type="button"
          onClick={() => onCancelTask(task.id)}
          title={`Cancel #${task.id}`}
          className="flex size-5 shrink-0 cursor-pointer items-center justify-center rounded text-faint opacity-0 transition-all group-hover:opacity-100 hover:bg-hover hover:text-red"
        >
          <X size={11} />
        </button>
      </div>
      {open && (
        <div className="mb-1 ml-[26px] mt-1 space-y-1 rounded border border-line/60 bg-card px-2 py-1.5">
          <div className="break-words text-text/85">{task.goal}</div>
          <div className="text-faint">Runs {task.when}</div>
          {latestRun && (
            <button
              type="button"
              onClick={() => onOpenRun(latestRun)}
              className="flex cursor-pointer items-center gap-1 rounded text-accent transition-colors hover:text-text"
            >
              <Orbit size={11} className="shrink-0" />
              <span>Open latest run</span>
              {latestRun.active && (
                <Loader2 size={11} className="shrink-0 animate-spin text-faint" />
              )}
              <ChevronRight size={11} className="shrink-0" />
            </button>
          )}
        </div>
      )}
    </div>
  );
}

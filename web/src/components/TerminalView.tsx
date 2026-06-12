import { useEffect, useRef, useState } from "react";
import { Loader2, SquareTerminal, X } from "lucide-react";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";

import { fetchJobTail, killJob, type JobTail } from "@/lib/api";
import type { TermRef } from "@/lib/term";
import { useTheme } from "@/lib/theme";
import type { Theme } from "@/lib/themes";
import { useDocumentVisible } from "@/lib/useDocumentVisible";

/** Job tabs poll the journal on one cadence whether their tab is visible or
 * not (it's one cheap localhost request), so the strip's spinner and the
 * output are always current the instant the tab shows. The poll does pause
 * while the WINDOW is hidden — nobody can see the spinner then — and resumes
 * with an immediate catch-up poll on visibility. */
const JOB_POLL_MS = 1200;

const FONT_MONO =
  'ui-monospace, "SF Mono", "Cascadia Mono", "JetBrains Mono", Menlo, Consolas, monospace';

function xtermTheme(theme: Theme) {
  return {
    background: theme.colors.canvas,
    foreground: theme.colors.text,
    cursor: theme.colors.bright,
    cursorAccent: theme.colors.canvas,
    selectionBackground: theme.colors.accent + "55",
  };
}

function b64bytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/**
 * A terminal tab's body — the machine-state panel, in both flavors of the one
 * reference type. A job terminal is a live read-only tail of an agent
 * command's journal (every bash call is a durable on-disk job, so this works
 * for running, finished, and resumed-from-history commands alike), with kill
 * while it runs and "open a shell here" to continue by hand. A shell terminal
 * is the user's own PTY on the host, attached over a WebSocket.
 */
export function TerminalView({
  term,
  active,
  onOpenShell,
  onBusy,
}: {
  term: TermRef;
  /** Visible in its pane — drives fit/focus; data keeps flowing regardless. */
  active: boolean;
  /** Open an interactive shell (a job terminal's "continue from here"). */
  onOpenShell?: (cwd?: string) => void;
  /** Live status for the tab strip's spinner (job terminals). */
  onBusy?: (busy: boolean) => void;
}) {
  return term.kind === "job" ? (
    <JobTerminal job={term.job} command={term.command} active={active} onOpenShell={onOpenShell} onBusy={onBusy} />
  ) : (
    <ShellTerminal id={term.id} active={active} />
  );
}

/**
 * The xterm instance for one tab: created once, refit on size/visibility
 * changes, repainted on theme switches. Returns refs the data effects feed.
 */
function useXterm(active: boolean, opts: { readOnly: boolean; convertEol: boolean }) {
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const theme = useTheme();
  const themeRef = useRef(theme);
  themeRef.current = theme;
  const [ready, setReady] = useState(0);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const term = new Terminal({
      convertEol: opts.convertEol,
      disableStdin: opts.readOnly,
      fontFamily: FONT_MONO,
      fontSize: 12,
      lineHeight: 1.25,
      scrollback: 8000,
      theme: xtermTheme(themeRef.current),
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(el);
    termRef.current = term;
    fitRef.current = fit;
    setReady((r) => r + 1);

    const refit = () => {
      // A hidden pane has no size; fitting then would corrupt the geometry.
      if (el.clientWidth > 0 && el.clientHeight > 0) fit.fit();
    };
    refit();
    const ro = new ResizeObserver(refit);
    ro.observe(el);
    return () => {
      ro.disconnect();
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
    };
    // The mode options are fixed per tab (a reference never changes kind).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const t = termRef.current;
    if (t) t.options.theme = xtermTheme(theme);
  }, [theme, ready]);

  // Becoming visible: the pane just got real dimensions — fit to them.
  useEffect(() => {
    const el = containerRef.current;
    if (active && el && el.clientWidth > 0 && el.clientHeight > 0) {
      fitRef.current?.fit();
    }
  }, [active, ready]);

  return { containerRef, termRef, ready };
}

function JobTerminal({
  job,
  command,
  active,
  onOpenShell,
  onBusy,
}: {
  job: string;
  command?: string;
  active: boolean;
  onOpenShell?: (cwd?: string) => void;
  onBusy?: (busy: boolean) => void;
}) {
  const { containerRef, termRef, ready } = useXterm(active, {
    readOnly: true,
    convertEol: true,
  });
  const [tail, setTail] = useState<JobTail | null>(null);
  const [error, setError] = useState("");
  const onBusyRef = useRef(onBusy);
  onBusyRef.current = onBusy;

  const running = tail?.status === "running";
  const docVisible = useDocumentVisible();

  // The journal offset survives visibility pauses (a re-run must resume where
  // the tail left off, not re-backfill and duplicate output in the terminal)
  // but resets when the tab is retargeted to a different job.
  const offsetRef = useRef(-1); // -1: first poll backfills the recent window
  const offsetJobRef = useRef(job);
  if (offsetJobRef.current !== job) {
    offsetJobRef.current = job;
    offsetRef.current = -1;
  }

  useEffect(() => {
    if (!ready || !docVisible) return;
    let stop = false;
    let timer = 0;
    const poll = () => {
      fetchJobTail(job, offsetRef.current)
        .then((t) => {
          if (stop) return;
          setError("");
          setTail(t);
          if (t.data) termRef.current?.write(b64bytes(t.data));
          offsetRef.current = t.offset;
          if (t.status === "running") {
            // Behind by more than one chunk: catch up immediately.
            timer = window.setTimeout(poll, t.offset < t.size ? 0 : JOB_POLL_MS);
          }
        })
        .catch((e: unknown) => {
          if (stop) return;
          setError(e instanceof Error ? e.message : String(e));
          timer = window.setTimeout(poll, JOB_POLL_MS);
        });
    };
    poll();
    return () => {
      stop = true;
      window.clearTimeout(timer);
    };
  }, [job, ready, docVisible, termRef]);

  useEffect(() => {
    onBusyRef.current?.(running);
  }, [running]);

  const statusText = !tail
    ? ""
    : tail.status === "running"
      ? "running"
      : tail.status === "killed"
        ? "killed"
        : `exit ${tail.exit_code}`;

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <div className="flex h-9 shrink-0 select-none items-center gap-2 border-b border-line/70 px-3">
        <SquareTerminal size={13} className="shrink-0 text-muted" />
        <span className="min-w-0 truncate font-mono text-[11.5px] text-bright">
          {tail?.command.split("\n")[0] || command || job}
        </span>
        <span className="min-w-0 flex-1 truncate text-[11px] text-faint">
          {job}
          {tail?.cwd ? ` · ${tail.cwd}` : ""}
        </span>
        {running && <Loader2 size={12} className="shrink-0 animate-spin text-faint" />}
        {statusText && !running && (
          <span
            className={`shrink-0 text-[11px] ${
              tail?.status === "exited" && tail.exit_code === 0 ? "text-faint" : "text-red"
            }`}
          >
            {statusText}
          </span>
        )}
        {running && (
          <button
            type="button"
            title={`Kill ${job}`}
            onClick={() => killJob(job).catch(() => {})}
            className="flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-md text-muted transition-colors hover:bg-hover hover:text-red"
          >
            <X size={12} />
          </button>
        )}
        {onOpenShell && (
          <button
            type="button"
            title="Open an interactive shell in this directory"
            onClick={() => onOpenShell(tail?.cwd)}
            className="flex h-6 shrink-0 cursor-pointer items-center rounded-md px-2 text-[11px] text-muted transition-colors hover:bg-hover hover:text-text"
          >
            Shell here
          </button>
        )}
      </div>
      {error && (
        <div className="shrink-0 border-b border-line/60 px-3 py-1.5 text-[11.5px] text-red">
          {error}
        </div>
      )}
      <div ref={containerRef} className="min-h-0 min-w-0 flex-1 bg-canvas px-2 py-1" />
    </div>
  );
}

function ShellTerminal({
  id,
  active,
}: {
  id: string;
  active: boolean;
}) {
  const { containerRef, termRef, ready } = useXterm(active, {
    readOnly: false,
    convertEol: false,
  });
  const [closed, setClosed] = useState(false);

  useEffect(() => {
    const term = termRef.current;
    if (!ready || !term) return;
    const proto = location.protocol === "https:" ? "wss" : "ws";
    const ws = new WebSocket(
      `${proto}://${location.host}/api/terminals/${encodeURIComponent(id)}/ws`,
    );
    ws.binaryType = "arraybuffer";
    setClosed(false);

    ws.onmessage = (ev) => {
      if (ev.data instanceof ArrayBuffer) {
        term.write(new Uint8Array(ev.data));
      }
    };
    ws.onclose = () => setClosed(true);

    const encoder = new TextEncoder();
    const data = term.onData((d) => {
      if (ws.readyState === WebSocket.OPEN) ws.send(encoder.encode(d));
    });
    const sendSize = (cols: number, rows: number) => {
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ resize: { cols, rows } }));
      }
    };
    const resize = term.onResize(({ cols, rows }) => sendSize(cols, rows));
    ws.onopen = () => sendSize(term.cols, term.rows);

    return () => {
      data.dispose();
      resize.dispose();
      ws.close();
    };
  }, [id, ready, termRef]);

  // The keyboard follows visibility: an activated shell tab is for typing.
  useEffect(() => {
    if (active && ready) termRef.current?.focus();
  }, [active, ready, termRef]);

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      {closed && (
        <div className="shrink-0 border-b border-line/60 px-3 py-1 text-[11px] text-faint">
          shell ended
        </div>
      )}
      <div ref={containerRef} className="min-h-0 min-w-0 flex-1 bg-canvas px-2 py-1" />
    </div>
  );
}

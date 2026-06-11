import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  AppWindow,
  Bell,
  Brain,
  Check,
  ChevronDown,
  Circle,
  CircleHelp,
  Clock,
  ListTodo,
  Loader2,
  Orbit,
  Pencil,
  Repeat,
  SquareTerminal,
  X,
} from "lucide-react";

import { Highlight, Markdown } from "./Markdown";
import { PartImages } from "./PartImages";
import { argsPreview } from "@/lib/format";
import { detailsSurface, type Surface } from "@/lib/surface";
import type { ChatState, TranscriptItem } from "@/lib/transcript";
import type { ContentBlock, ToolCall } from "@/lib/types";
import { useAutosize } from "@/lib/useAutosize";

/** Hooks the transcript needs from its host (sub-agent tab opening). */
export interface TranscriptHooks {
  /** Live sub-agent transcripts, for chip status (running / done). */
  children?: Record<string, ChatState>;
  /** Open a sub-agent's transcript panel. */
  onOpenChild?: (session: string, label: string) => void;
  /** Open a surface (a shown file) in a panel beside the chat. */
  onOpenSurface?: (surface: Surface) => void;
  /**
   * Rewind-and-edit (Cursor's edit-a-previous-turn): clicking a past user
   * message opens it for editing; submitting forks the session at that point
   * and resubmits. Absent in sub-agent panels, which are read-only.
   */
  edit?: TranscriptEditHooks;
}

export interface TranscriptEditHooks {
  /** Item id currently being edited inline, if any. */
  editingId: number | null;
  /** Whether edit can start now (seam connected). */
  canEdit: boolean;
  onStart: (id: number) => void;
  onCancel: () => void;
  onSubmit: (id: number, text: string) => void;
}

/**
 * The transcript, rendered the way Cursor's agent panel renders it: the user
 * prompt as a quiet card, plain prose answers, tool activity as dim one-line
 * summaries, terminal commands and file edits as bordered cards with tinted
 * output / diff lines. Delegations render as clickable sub-agent tabs.
 */
export function ChatView({
  items,
  hooks,
}: {
  items: TranscriptItem[];
  hooks?: TranscriptHooks;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const pinnedRef = useRef(true);

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
    <div
      ref={scrollRef}
      onScroll={onScroll}
      className="min-h-0 flex-1 overflow-y-auto"
    >
      <TranscriptList items={items} hooks={hooks} />
    </div>
  );
}

/**
 * The bare item column — shared between the main transcript and a sub-agent
 * panel, so a child's chat renders with exactly the parent's vocabulary.
 */
export function TranscriptList({
  items,
  hooks,
}: {
  items: TranscriptItem[];
  hooks?: TranscriptHooks;
}) {
  return (
    <div className="mx-auto w-full max-w-4xl space-y-2 px-3.5 py-4">
      {items.map((item) => (
        <Item key={item.id} item={item} hooks={hooks} />
      ))}
    </div>
  );
}

function Item({ item, hooks }: { item: TranscriptItem; hooks?: TranscriptHooks }) {
  switch (item.kind) {
    case "user":
      return <UserItem item={item} edit={hooks?.edit} />;

    case "assistant":
      return (
        <div className="break-words py-1">
          <Markdown content={item.text} streaming={item.streaming} />
          {!item.streaming && item.stopReason && item.stopReason !== "answered" && (
            <div className="mt-1 text-[12px] text-warn">
              stopped: {item.stopReason}
            </div>
          )}
        </div>
      );

    case "thinking":
      return <ThinkingBlock item={item} />;

    case "tool":
      return <ToolItem item={item} hooks={hooks} />;

    case "subagent": {
      const child = hooks?.children?.[item.session];
      return (
        <SubagentChip
          label={item.label}
          status={child ? childActivity(child) : ""}
          running={child?.turnActive ?? false}
          failed={false}
          onOpen={
            hooks?.onOpenChild
              ? () => hooks.onOpenChild?.(item.session, item.label)
              : undefined
          }
        />
      );
    }

    case "queued":
      return <div className="text-muted">Queued · {item.text}</div>;

    case "interrupted":
      return <div className="text-muted">Stopped</div>;

    case "notice":
      // The agent's voice between turns (outbox): a scheduled firing or
      // finished background work speaking up, ambient like Cursor's rows.
      return (
        <div className="flex items-start gap-2 rounded-md border border-line/60 bg-card/60 px-3 py-2">
          <Bell size={12} className="mt-1 shrink-0 text-muted" />
          <span className="min-w-0 flex-1 whitespace-pre-wrap break-words text-text">
            {item.text}
          </span>
        </div>
      );

    case "error":
      return (
        <div className="whitespace-pre-wrap break-words text-[12px] text-red">
          {item.message}
          {item.retryable && (
            <span className="text-faint"> — send again to retry</span>
          )}
        </div>
      );

    default: {
      const never: never = item;
      void never;
      return null;
    }
  }
}

/** Inline image attachments shown in a user prompt bubble. */
/**
 * A past prompt. Sticky like Cursor: the card pins to the top of the scroll
 * area while its turn streams beneath it; the next prompt displaces it. The
 * full-bleed canvas wrapper masks content scrolling past behind the card's
 * rounded corners.
 *
 * With edit hooks, double-clicking (or the hover pencil) turns the card into
 * an inline composer — submitting forks the session at this message and
 * resubmits the edited text (Cursor's edit-a-previous-turn). Works mid-turn
 * too: the fork cancels the in-flight turn, like Cursor's checkpoint restore.
 */
function UserItem({
  item,
  edit,
}: {
  item: Extract<TranscriptItem, { kind: "user" }>;
  edit?: TranscriptEditHooks;
}) {
  const editing = edit?.editingId === item.id;
  return (
    <div className="sticky top-0 z-10 -mx-3.5 bg-canvas px-3.5 pt-1 pb-1">
      {editing && edit ? (
        <UserEditCard item={item} edit={edit} />
      ) : (
        <div
          onDoubleClick={
            edit?.canEdit ? () => edit.onStart(item.id) : undefined
          }
          title={edit?.canEdit ? "Double-click to edit and resubmit from here" : undefined}
          className="group relative rounded-md border border-line/70 bg-card px-3 py-2 text-bright"
        >
          {item.text && (
            <div className="whitespace-pre-wrap break-words">{item.text}</div>
          )}
          <UserAttachments parts={item.parts} />
          {edit?.canEdit && (
            <button
              type="button"
              onClick={() => edit.onStart(item.id)}
              title="Edit and resubmit from here"
              className="absolute right-1.5 top-1.5 flex size-6 cursor-pointer items-center justify-center rounded bg-card text-faint opacity-0 transition-all group-hover:opacity-100 hover:bg-hover hover:text-text"
            >
              <Pencil size={12} />
            </button>
          )}
        </div>
      )}
    </div>
  );
}

/** The user card in edit mode: Enter resubmits (forking here), Esc cancels. */
function UserEditCard({
  item,
  edit,
}: {
  item: Extract<TranscriptItem, { kind: "user" }>;
  edit: TranscriptEditHooks;
}) {
  const [draft, setDraft] = useState(item.text);
  const taRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    const ta = taRef.current;
    if (!ta) return;
    ta.focus();
    ta.setSelectionRange(ta.value.length, ta.value.length);
  }, []);

  // Autosize to content so the whole message stays visible while editing.
  useAutosize(taRef, draft);

  const submit = () => {
    if (draft.trim()) edit.onSubmit(item.id, draft);
  };

  return (
    <div className="rounded-md border border-accent/50 bg-card px-3 py-2 text-bright ring-1 ring-accent/30">
      <textarea
        ref={taRef}
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            submit();
          }
          if (e.key === "Escape") {
            e.preventDefault();
            edit.onCancel();
          }
        }}
        rows={1}
        className="block w-full resize-none bg-transparent leading-relaxed text-bright outline-none"
      />
      <UserAttachments parts={item.parts} />
      <div className="mt-1.5 flex items-center justify-between">
        <span className="text-[11px] text-faint select-none">
          resubmits from here — the turns below are discarded
        </span>
        <span className="flex items-center gap-1.5">
          <button
            type="button"
            onClick={edit.onCancel}
            title="Cancel (esc)"
            className="cursor-pointer rounded-md border border-line px-2 py-0.5 text-[12px] text-muted transition-colors hover:text-text"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={submit}
            disabled={!draft.trim()}
            title="Resubmit (enter)"
            className="cursor-pointer rounded-md bg-btn px-2 py-0.5 text-[12px] font-medium text-canvas transition-opacity hover:opacity-90 disabled:cursor-default disabled:opacity-30"
          >
            Send
          </button>
        </span>
      </div>
    </div>
  );
}

function UserAttachments({ parts }: { parts?: ContentBlock[] }) {
  return (
    <PartImages
      parts={parts}
      wrap="mt-2 flex flex-wrap gap-2"
      className="max-h-48 max-w-full rounded-md border border-line/60 object-contain"
    />
  );
}

/**
 * The model's reasoning as Cursor renders it: a quiet `Thinking ⌄` header
 * (shimmering while it streams) over dim prose, collapsible once you've
 * read enough. Always expanded while streaming so thought scrolls live.
 */
function ThinkingBlock({
  item,
}: {
  item: Extract<TranscriptItem, { kind: "thinking" }>;
}) {
  const [collapsed, setCollapsed] = useState(false);
  const open = item.streaming || !collapsed;

  return (
    <div className="py-0.5">
      <button
        type="button"
        onClick={() => setCollapsed(!collapsed)}
        className="flex cursor-pointer items-center gap-1 text-muted transition-colors hover:text-text"
      >
        <span className={item.streaming ? "shimmer" : ""}>Thinking</span>
        <ChevronDown
          size={13}
          className={`text-faint transition-transform ${open ? "" : "-rotate-90"}`}
        />
      </button>
      {open && (
        <div className="mt-1 whitespace-pre-wrap break-words text-muted">
          {item.text.trim()}
        </div>
      )}
    </div>
  );
}

type ToolTranscriptItem = Extract<TranscriptItem, { kind: "tool" }>;

/** Route each tool to its Cursor-style rendering. */
function ToolItem({ item, hooks }: { item: ToolTranscriptItem; hooks?: TranscriptHooks }) {
  switch (item.call.Name) {
    case "bash":
      return <TerminalCard item={item} />;
    case "edit":
    case "write":
      return <DiffCard item={item} />;
    case "plan":
      return <PlanItem item={item} />;
    case "delegate":
      return <DelegateChip item={item} hooks={hooks} />;
    case "show":
      return <ShowChip item={item} hooks={hooks} />;
    case "remember":
      return <MemoryCard item={item} />;
    default:
      return <SummaryRow item={item} />;
  }
}

/**
 * A remember call as Cursor renders a saved memory: a quiet bordered card
 * with a caption row (icon + "Memory" + state) over the fact itself, instead
 * of a raw `remember fact=…` args dump.
 */
function MemoryCard({ item }: { item: ToolTranscriptItem }) {
  const running = !item.result;
  const failed = item.result?.IsError ?? false;
  const fact = str(args(item.call).fact);

  return (
    <div className="rounded-md border border-line/60 bg-card/60 px-3 py-2">
      <div className="flex items-center gap-1.5">
        <Brain size={12} className="shrink-0 text-muted" />
        <span className="text-[11px] font-medium uppercase tracking-wider text-faint">
          Memory
        </span>
        <span className="flex-1" />
        {running && (
          <Loader2 size={12} className="shrink-0 animate-spin text-faint" />
        )}
        {failed && <X size={12} className="shrink-0 text-red" />}
        {!running && !failed && (
          <span className="flex items-center gap-1 text-[11px] text-faint">
            <Check size={12} className="shrink-0 text-green" />
            saved
          </span>
        )}
      </div>
      <div className="mt-1 whitespace-pre-wrap break-words text-[12.5px] leading-relaxed text-text">
        {fact}
      </div>
      {failed && item.result && (
        <div className="mt-1 truncate text-[11.5px] text-red/80">
          {item.result.Content.slice(0, ERROR_PREVIEW_MAX)}
        </div>
      )}
    </div>
  );
}

/**
 * A show call: the agent opened a file in a panel beside the chat. The row
 * stays clickable forever — on a resumed session it is what re-opens the
 * surface (the reference rides the result's Details through replay).
 */
function ShowChip({ item, hooks }: { item: ToolTranscriptItem; hooks?: TranscriptHooks }) {
  const running = !item.result;
  const failed = item.result?.IsError ?? false;
  const surface = item.result && !failed ? detailsSurface(item.result) : undefined;
  const a = args(item.call);
  const label = surface?.title || str(a.title) || basename(str(a.path));

  if (failed) {
    return (
      <div className="text-muted">
        <div className="flex min-w-0 items-center gap-1.5">
          <X size={12} className="shrink-0 text-red" />
          <span className="shrink-0">Present</span>
          <span className="truncate font-mono text-[11.5px] text-muted/80">
            {str(a.path)}
          </span>
        </div>
        {item.result && (
          <div className="truncate pl-[18px] text-[11.5px] text-red/80">
            {item.result.Content.slice(0, ERROR_PREVIEW_MAX)}
          </div>
        )}
      </div>
    );
  }

  const open = surface && hooks?.onOpenSurface
    ? () => hooks.onOpenSurface?.(surface)
    : undefined;
  return (
    <button
      type="button"
      onClick={open}
      disabled={!open}
      title={open ? "Open in a panel" : undefined}
      className={`-mx-2 flex w-[calc(100%+1rem)] min-w-0 items-center gap-2 rounded-md px-2 py-1 text-left transition-colors ${
        open ? "cursor-pointer hover:bg-card" : "cursor-default"
      }`}
    >
      {running ? (
        <Loader2 size={13} className="shrink-0 animate-spin text-faint" />
      ) : (
        <AppWindow size={13} className="shrink-0 text-muted" />
      )}
      <span className="min-w-0 truncate text-text">{label}</span>
      <span className="min-w-0 truncate font-mono text-[11px] text-faint">
        {surface?.path ?? str(a.path)}
      </span>
    </button>
  );
}

/**
 * A delegation as Cursor renders a sub-agent: a quiet two-line row — icon,
 * task title, muted backend label — over a dim live-activity line (the
 * child's latest action while it runs, its final word once it's done).
 * Clicking opens the child's own chat.
 */
function DelegateChip({ item, hooks }: { item: ToolTranscriptItem; hooks?: TranscriptHooks }) {
  const a = args(item.call);
  const label = str(a.instruction).split("\n")[0] || "Sub-agent task";
  const sid = item.childSession;
  const child = sid ? hooks?.children?.[sid] : undefined;
  const running = !item.result;
  const failed = item.result?.IsError ?? false;
  const status = running
    ? (child && childActivity(child)) || "Starting"
    : firstLine(item.result?.Content ?? "");
  return (
    <SubagentChip
      label={label}
      meta={str(a.backend)}
      status={status}
      running={running}
      failed={failed}
      onOpen={
        sid && hooks?.onOpenChild
          ? () => hooks.onOpenChild?.(sid, label)
          : undefined
      }
    />
  );
}

/**
 * The child's latest visible action as one dim line — what Cursor shows
 * under a running sub-agent ("Testing message edit functionality").
 */
function childActivity(child: ChatState): string {
  for (let i = child.items.length - 1; i >= 0; i--) {
    const it = child.items[i];
    switch (it.kind) {
      case "tool": {
        if (it.call.Name === "bash") {
          const a = args(it.call);
          return str(a.description) || str(a.command) || "Running a command";
        }
        const { verb, arg } = summary(it.call);
        return arg ? `${verb} ${arg}` : verb;
      }
      case "thinking":
        return "Thinking";
      case "assistant": {
        const line = firstLine(it.text);
        if (line) return line;
        continue;
      }
      case "user":
      case "queued":
      case "interrupted":
      case "error":
      case "notice":
      case "subagent":
        continue;
      default: {
        const never: never = it;
        void never;
        continue;
      }
    }
  }
  return "";
}

function firstLine(text: string): string {
  return text.trim().split("\n")[0] ?? "";
}

/** The row itself: `[orbit] title  backend` over a dim status line. */
function SubagentChip({
  label,
  meta,
  status,
  running,
  failed,
  onOpen,
}: {
  label: string;
  meta?: string;
  status?: string;
  running: boolean;
  failed: boolean;
  onOpen?: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onOpen}
      disabled={!onOpen}
      title={onOpen ? "Open sub-agent" : "Sub-agent transcript unavailable"}
      className={`-mx-2 block w-[calc(100%+1rem)] rounded-md px-2 py-1 text-left transition-colors ${
        onOpen ? "cursor-pointer hover:bg-card" : "cursor-default"
      }`}
    >
      <span className="flex min-w-0 items-center gap-2">
        <Orbit
          size={13}
          className={`shrink-0 text-muted ${running ? "animate-[spin_3s_linear_infinite]" : ""}`}
        />
        <span className="min-w-0 truncate text-text">{label}</span>
        {meta && (
          <span className="shrink-0 text-[11.5px] text-faint">{meta}</span>
        )}
        {failed && <X size={12} className="ml-auto shrink-0 text-red" />}
      </span>
      {status && (
        <span
          className={`mt-0.5 block truncate pl-[21px] text-[12px] ${
            failed ? "text-red/80" : "text-faint"
          } ${running ? "shimmer" : ""}`}
        >
          {status}
        </span>
      )}
    </button>
  );
}

const ERROR_PREVIEW_MAX = 400;

function str(v: unknown): string {
  return typeof v === "string" ? v : "";
}

function basename(p: string): string {
  return p.split("/").pop() || p;
}

function args(call: ToolCall): Record<string, unknown> {
  return typeof call.Args === "object" && call.Args !== null
    ? (call.Args as Record<string, unknown>)
    : {};
}

/** Past-tense one-liners, the way Cursor narrates tool use. */
function summary(call: ToolCall): { verb: string; arg: string } {
  const a = args(call);
  switch (call.Name) {
    case "read":
      return { verb: "Read", arg: basename(str(a.path)) };
    case "ls":
      return { verb: "Listed", arg: str(a.path) || "." };
    case "find":
      return { verb: "Searched files", arg: str(a.pattern) };
    case "grep":
      return { verb: "Grepped", arg: str(a.pattern) };
    case "fetch":
      return { verb: "Fetched", arg: str(a.url) };
    case "await":
      return { verb: "Waited on job", arg: str(a.id) };
    case "jobs":
      return { verb: "Listed jobs", arg: "" };
    case "undo":
      return { verb: "Undid edit", arg: basename(str(a.path)) };
    default:
      return { verb: call.Name, arg: argsPreview(call, 80) };
  }
}

/** One dim line per quiet tool call, e.g. `Read web/src/App.tsx`. */
function SummaryRow({ item }: { item: ToolTranscriptItem }) {
  const running = !item.result;
  const failed = item.result?.IsError ?? false;
  const { verb, arg } = summary(item.call);

  return (
    <div className="text-muted">
      <div className="flex min-w-0 items-center gap-1.5">
        {running && (
          <Loader2 size={12} className="shrink-0 animate-spin text-faint" />
        )}
        {failed && <X size={12} className="shrink-0 text-red" />}
        <span className="shrink-0">{verb}</span>
        {arg && (
          <span className="truncate font-mono text-[11.5px] text-muted/80">
            {arg}
          </span>
        )}
      </div>
      {failed && item.result && (
        <div className="truncate pl-[18px] text-[11.5px] text-red/80">
          {item.result.Content.slice(0, ERROR_PREVIEW_MAX)}
        </div>
      )}
    </div>
  );
}

const TERMINAL_TAIL_LINES = 12;

const BACKGROUNDED_REPORT =
  /\s*Command is (?:still )?running in the background as job ([\w.-]+)[\s\S]*$/;

/**
 * Trim the kernel's backgrounding boilerplate ("Command is running in the
 * background as job… await… kill…") to a short badge; the model needs those
 * instructions, the user just needs to know it kept running.
 */
function trimJobReport(output: string): { text: string; job?: string } {
  const m = output.match(BACKGROUNDED_REPORT);
  if (!m) return { text: output };
  const rest = output.slice(0, m.index).trimEnd();
  return { text: rest === "(no output yet)" ? "" : rest, job: m[1] };
}

/**
 * A bash call as Cursor's terminal card: `[icon] Description command` in the
 * header (the description is the tool's display-only arg), dim output below.
 */
function TerminalCard({ item }: { item: ToolTranscriptItem }) {
  const [open, setOpen] = useState(true);
  const [stopped, setStopped] = useState(false);
  const running = !item.result;
  const failed = item.result?.IsError ?? false;
  const a = args(item.call);
  const command = str(a.command) || argsPreview(item.call);
  const description = str(a.description);
  const { text: output, job } = trimJobReport(item.result?.Content.trimEnd() ?? "");
  const tail = output.split("\n").slice(-TERMINAL_TAIL_LINES).join("\n");

  const stopJob = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (!job) return;
    try {
      const res = await fetch(`/api/jobs/${encodeURIComponent(job)}/kill`, {
        method: "POST",
      });
      if (res.ok) setStopped(true);
    } catch {
      // Gateway unreachable; the badge keeps spinning, retry is a click away.
    }
  };

  return (
    <div className="overflow-hidden rounded-lg border border-line/80">
      <button
        type="button"
        onClick={() => setOpen(!open)}
        className="flex w-full cursor-pointer items-center gap-2 px-3 py-1.5 text-left"
      >
        <SquareTerminal size={13} className="shrink-0 text-muted" />
        <span className="min-w-0 flex-1 truncate">
          {description && <span className="text-muted">{description} </span>}
          <span className="font-mono text-[11.5px] text-faint">{command}</span>
        </span>
        {running && (
          <Loader2 size={12} className="shrink-0 animate-spin text-faint" />
        )}
        {failed && <X size={12} className="shrink-0 text-red" />}
      </button>
      {open && (tail || job) && (
        <div className="border-t border-line/60">
          {tail && (
            <pre className="max-h-44 overflow-y-auto whitespace-pre-wrap break-words px-3 py-2 font-mono text-[11.5px] leading-relaxed text-muted/80">
              {tail}
            </pre>
          )}
          {job && (
            <div className="flex items-center gap-1.5 px-3 pb-2 pt-1 text-[11.5px] text-faint">
              {stopped ? (
                <>
                  <X size={11} />
                  stopped · {job}
                </>
              ) : (
                <>
                  <Loader2 size={11} className="animate-spin" />
                  running in background · {job}
                  <button
                    type="button"
                    onClick={stopJob}
                    title={`Stop ${job}`}
                    className="ml-1 flex size-4 cursor-pointer items-center justify-center rounded text-faint transition-colors hover:bg-hover hover:text-red"
                  >
                    <X size={11} />
                  </button>
                </>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

/* ------------------------------------------------------------------ */
/* Plan: op:add renders as Cursor's to-do card (one row per node, with */
/* its trigger); op:update is one quiet status line; op:show is dim.   */
/* ------------------------------------------------------------------ */

type PlanNodeArg = {
  goal?: string;
  when?: { after?: string; every?: string; onDeps?: boolean };
  do?: { shell?: string; notify?: string; ask?: boolean };
};

function planWhen(n: PlanNodeArg): string {
  if (n.when?.every) return `every ${n.when.every}`;
  if (n.when?.after) return `in ${n.when.after}`;
  if (n.when?.onDeps) return "on deps";
  return "";
}

function PlanNodeIcon({ node }: { node: PlanNodeArg }) {
  if (node.do?.notify) return <Bell size={12} className="text-muted" />;
  if (node.do?.shell) return <SquareTerminal size={12} className="text-muted" />;
  if (node.do?.ask) return <CircleHelp size={12} className="text-muted" />;
  if (node.when?.every) return <Repeat size={12} className="text-muted" />;
  if (node.when?.after) return <Clock size={12} className="text-muted" />;
  return <Circle size={11} className="text-faint" />;
}

const PLAN_STATUS_ICON: Record<string, ReactNode> = {
  done: <Check size={12} className="text-green" />,
  failed: <X size={12} className="text-red" />,
  cancelled: <X size={12} className="text-faint" />,
  blocked: <CircleHelp size={12} className="text-warn" />,
  active: <Circle size={11} className="text-accent" />,
  pending: <Circle size={11} className="text-faint" />,
};

function PlanItem({ item }: { item: ToolTranscriptItem }) {
  const running = !item.result;
  const failed = item.result?.IsError ?? false;
  const a = args(item.call);

  if (failed) {
    return (
      <div className="flex min-w-0 items-center gap-1.5 text-muted">
        <X size={12} className="shrink-0 text-red" />
        <span>Plan update failed</span>
      </div>
    );
  }

  if (a.op === "add" && Array.isArray(a.nodes) && a.nodes.length > 0) {
    const nodes = a.nodes as PlanNodeArg[];
    return (
      <div className="overflow-hidden rounded-lg border border-line/80">
        <div className="flex items-center gap-2 px-3 py-1.5 text-muted">
          <ListTodo size={13} className="shrink-0" />
          <span>Updated plan</span>
          {running && (
            <Loader2 size={12} className="ml-auto shrink-0 animate-spin text-faint" />
          )}
        </div>
        <div className="space-y-1 border-t border-line/60 px-3 py-2">
          {nodes.map((n, i) => (
            <div key={i} className="flex min-w-0 items-start gap-2">
              <span className="mt-1 shrink-0">
                <PlanNodeIcon node={n} />
              </span>
              <span className="min-w-0 flex-1 break-words text-text/90">
                {n.goal}
                {planWhen(n) && (
                  <span className="text-faint"> · {planWhen(n)}</span>
                )}
              </span>
            </div>
          ))}
        </div>
      </div>
    );
  }

  if (a.op === "update") {
    const status = String(a.status ?? "");
    const icon = status
      ? (PLAN_STATUS_ICON[status] ?? <Circle size={11} className="text-faint" />)
      : <Repeat size={12} className="text-muted" />;
    const verb = status ? `Marked plan #${a.node} ${status}` : `Plan #${a.node} recurred`;
    const outcome = str(a.outcome);
    return (
      <div className="flex min-w-0 items-center gap-1.5 text-muted">
        {running ? (
          <Loader2 size={12} className="shrink-0 animate-spin text-faint" />
        ) : (
          <span className="shrink-0">{icon}</span>
        )}
        <span className="shrink-0">{verb}</span>
        {outcome && <span className="truncate text-faint">— {outcome}</span>}
      </div>
    );
  }

  return (
    <div className="flex min-w-0 items-center gap-1.5 text-muted">
      {running && <Loader2 size={12} className="shrink-0 animate-spin text-faint" />}
      <span>Reviewed plan</span>
    </div>
  );
}

type DiffLine = { kind: "add" | "del" | "ctx" | "gap"; text: string };

const DIFF_MAX_LINES = 40;

/**
 * Parse the edit tool's diff (Details.diff): `+NN text` / `-NN text` /
 * ` NN text` rows with `...` markers for collapsed context. Line numbers are
 * the tool's framing for the model; the card shows bare code like Cursor.
 */
function parseEditDiff(diff: string): DiffLine[] {
  return diff.split("\n").map((line): DiffLine => {
    if (line.trim() === "...") return { kind: "gap", text: "" };
    const m = line.match(/^([+\- ])\s*\d+(?: (.*))?$/);
    if (!m) return { kind: "ctx", text: line };
    const kind = m[1] === "+" ? "add" : m[1] === "-" ? "del" : "ctx";
    return { kind, text: m[2] ?? "" };
  });
}

function diffLines(item: ToolTranscriptItem): DiffLine[] {
  if (item.call.Name === "write") {
    // A write is all additions; preview the head of the new content.
    return str(args(item.call).content)
      .split("\n")
      .map((text) => ({ kind: "add" as const, text }));
  }
  const details = item.result?.Details;
  if (typeof details === "object" && details !== null && "diff" in details) {
    const d = (details as { diff?: unknown }).diff;
    if (typeof d === "string" && d) return parseEditDiff(d);
  }
  return [];
}

const DIFF_LINE_BG: Record<DiffLine["kind"], string> = {
  add: "bg-add-bg",
  del: "bg-del-bg",
  ctx: "",
  gap: "",
};

/** An edit/write call as Cursor's diff card: filename + counts, tinted lines. */
function DiffCard({ item }: { item: ToolTranscriptItem }) {
  const [open, setOpen] = useState(true);
  const running = !item.result;
  const failed = item.result?.IsError ?? false;
  const name = basename(str(args(item.call).path));
  const lines = diffLines(item);
  const adds = lines.filter((l) => l.kind === "add").length;
  const dels = lines.filter((l) => l.kind === "del").length;

  return (
    <div className="overflow-hidden rounded-md border border-line/80">
      <button
        type="button"
        onClick={() => setOpen(!open)}
        className="flex w-full cursor-pointer items-center gap-2 bg-card px-3 py-1.5 text-left"
      >
        <span className="size-1.5 shrink-0 rounded-full bg-accent" />
        <span className="min-w-0 truncate text-[12px] text-text">{name}</span>
        {!running && !failed && (
          <span className="shrink-0 font-mono text-[11px]">
            {adds > 0 && <span className="text-green">+{adds}</span>}{" "}
            {dels > 0 && <span className="text-red">-{dels}</span>}
          </span>
        )}
        <span className="flex-1" />
        {running && (
          <Loader2 size={12} className="shrink-0 animate-spin text-faint" />
        )}
        {failed && <X size={12} className="shrink-0 text-red" />}
      </button>
      {open && !running && !failed && lines.length > 0 && (
        <div className="max-h-56 overflow-y-auto py-1 font-mono text-[11.5px] leading-[1.5]">
          {lines.slice(0, DIFF_MAX_LINES).map((l, i) =>
            l.kind === "gap" ? (
              <div key={i} className="px-3 text-faint select-none">
                ⋯
              </div>
            ) : (
              <div
                key={i}
                className={`whitespace-pre px-3 ${DIFF_LINE_BG[l.kind]} ${
                  l.kind === "ctx" ? "text-faint" : "text-text/90"
                }`}
              >
                {l.text ? <Highlight text={l.text} /> : " "}
              </div>
            ),
          )}
          {lines.length > DIFF_MAX_LINES && (
            <div className="px-3 text-faint select-none">⋯</div>
          )}
        </div>
      )}
      {failed && item.result && (
        <div className="truncate px-3 py-1.5 font-mono text-[11.5px] text-red/80">
          {item.result.Content.slice(0, ERROR_PREVIEW_MAX)}
        </div>
      )}
    </div>
  );
}

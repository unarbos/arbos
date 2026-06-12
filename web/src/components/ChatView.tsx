import { memo, useEffect, useRef, useState, type ReactNode } from "react";
import {
  AppWindow,
  Bell,
  Brain,
  Check,
  ChevronDown,
  Circle,
  CircleHelp,
  Clock,
  FileText,
  Globe,
  ListTodo,
  Loader2,
  Maximize2,
  Pencil,
  Repeat,
  SquareTerminal,
  X,
} from "lucide-react";

import { CopyButton, Highlight, Markdown } from "./Markdown";
import { PartImages } from "./PartImages";
import { fetchJobTail, HttpError } from "@/lib/api";
import { argsPreview } from "@/lib/format";
import { detailsSurface, dirSurface, fileSurface, type Surface } from "@/lib/surface";
import { detailsJob, type TermRef } from "@/lib/term";
import type { ChatState, TranscriptItem } from "@/lib/transcript";
import type { Citation, ContentBlock, ToolCall } from "@/lib/types";
import { useAutosize } from "@/lib/useAutosize";

/** Hooks the transcript needs from its host (sub-agent tab opening). */
export interface TranscriptHooks {
  /** Live sub-agent transcripts, for chip status (running / done). */
  children?: Record<string, ChatState>;
  /** Open a sub-agent's transcript panel. */
  onOpenChild?: (session: string, label: string) => void;
  /** Open a surface (a shown file) in a panel beside the chat. */
  onOpenSurface?: (surface: Surface) => void;
  /** Open a terminal tab (a bash card's live job tail, or a shell). */
  onOpenTerminal?: (term: TermRef) => void;
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
  working,
  hooks,
}: {
  items: TranscriptItem[];
  /** The turn is live but nothing is visibly streaming — show the "working"
   *  line so the page reacts the instant a prompt is sent. */
  working?: boolean;
  hooks?: TranscriptHooks;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const pinnedRef = useRef(true);

  // Follow the tail while pinned, batched to one scroll per frame: streaming
  // deltas arrive faster than the display refreshes, and setting scrollTop
  // synchronously on each one forces extra layout work for frames nobody sees.
  useEffect(() => {
    const el = scrollRef.current;
    if (!el || !pinnedRef.current) return;
    const id = requestAnimationFrame(() => {
      el.scrollTop = el.scrollHeight;
    });
    return () => cancelAnimationFrame(id);
  }, [items, working]);

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
      <TranscriptList items={items} working={working} hooks={hooks} />
    </div>
  );
}

/**
 * The bare item column — shared between the main transcript and a sub-agent
 * panel, so a child's chat renders with exactly the parent's vocabulary.
 */
export function TranscriptList({
  items,
  working,
  hooks,
}: {
  items: TranscriptItem[];
  working?: boolean;
  hooks?: TranscriptHooks;
}) {
  // The heartbeat only shows when nothing is already moving: prose/thinking
  // streaming or a tool spinning carry their own motion, so a second line
  // under them would just be noise.
  const last = items[items.length - 1];
  const liveTail =
    (last?.kind === "assistant" && last.streaming) ||
    (last?.kind === "thinking" && last.streaming) ||
    (last?.kind === "tool" && !last.result);

  // …but a streaming prose/thinking tail that has stopped GROWING is not
  // actually moving: the model has gone on to compose a tool call's arguments
  // (e.g. a canvas's whole HTML body), which stream invisibly — the kernel
  // emits nothing between the last delta and the tool landing. The caret just
  // blinks on frozen text. So once the tail goes quiet, treat it as not-live
  // and let the heartbeat resurface, the way the TUI keeps a "Working" spinner
  // animating through the same gap. A running tool tail (its own spinner) is
  // genuine motion and is left alone.
  const stale = useStaleTail(last, working ?? false);

  // Each user prompt and the items under it form one turn wrapper. The
  // prompt card is sticky WITHIN its wrapper, so the next turn's arrival
  // pushes the previous card off-screen instead of layering on top of it —
  // only one pinned prompt is ever visible.
  const turns = groupTurns(items);

  return (
    <div className="mx-auto w-full max-w-4xl space-y-2 px-3.5 py-4">
      {turns.map((turn) => (
        <div key={turn[0].id} className="space-y-2">
          {turn.map((item) => (
            <Item key={item.id} item={item} hooks={hooks} />
          ))}
        </div>
      ))}
      {working && (!liveTail || stale) && <WorkingIndicator last={last} />}
    </div>
  );
}

/** Split the flat item list into turns: every user message starts a new
 *  group (a leading group without one holds any pre-prompt items). */
function groupTurns(items: TranscriptItem[]): TranscriptItem[][] {
  const turns: TranscriptItem[][] = [];
  for (const item of items) {
    if (item.kind === "user" || turns.length === 0) turns.push([item]);
    else turns[turns.length - 1].push(item);
  }
  return turns;
}

/** How long a streaming tail may go without growing before it counts as
 *  stalled — long enough to ignore normal between-token pauses, short enough
 *  that a tool-argument gap doesn't read as a freeze. */
const STALE_TAIL_MS = 1000;

/**
 * Whether the live tail has stopped producing output. While a turn is active
 * and the last item is a streaming assistant/thinking block, arm a timer that
 * fires once its text hasn't grown for STALE_TAIL_MS; every new delta (the
 * text changing) resets it, and a non-streaming tail clears it outright.
 */
function useStaleTail(last: TranscriptItem | undefined, working: boolean): boolean {
  const growing =
    last && (last.kind === "assistant" || last.kind === "thinking") && last.streaming
      ? last.text
      : null;
  const [stale, setStale] = useState(false);

  useEffect(() => {
    setStale(false);
    if (!working || growing === null) return;
    const id = window.setTimeout(() => setStale(true), STALE_TAIL_MS);
    return () => window.clearTimeout(id);
  }, [growing, working]);

  return stale;
}

/**
 * The between-steps heartbeat: the moment a prompt is sent (and again whenever
 * the model is deciding its next move with nothing streaming yet) a shimmering
 * line keeps the page alive, the way Cursor never leaves a sent message
 * sitting in silence. The copy leans on what just happened — a fresh prompt is
 * "Planning next moves", a lull mid-turn is "Working".
 */
function WorkingIndicator({ last }: { last?: TranscriptItem }) {
  const label = !last || last.kind === "user" ? "Planning next moves" : "Working";
  return (
    <div className="flex items-center gap-2 py-0.5 text-muted">
      <Loader2 size={13} className="shrink-0 animate-spin text-faint" />
      <span className="shimmer">{label}</span>
    </div>
  );
}

/**
 * One transcript row, memoized: a streaming delta replaces only the growing
 * item in the array, so every other row keeps its identity and bails out
 * here instead of re-rendering the whole transcript per token. Requires the
 * `hooks` object to be referentially stable across deltas (ChatTab builds it
 * with useMemo over refs) — rebuilding it inline would defeat the memo.
 */
const Item = memo(function Item({
  item,
  hooks,
}: {
  item: TranscriptItem;
  hooks?: TranscriptHooks;
}) {
  switch (item.kind) {
    case "user":
      return (
        <UserItem
          item={item}
          edit={hooks?.edit}
          onOpenSurface={hooks?.onOpenSurface}
        />
      );

    case "assistant":
      return (
        <div className="group/msg break-words py-1">
          <Markdown content={item.text} streaming={item.streaming} />
          {item.images && item.images.length > 0 && (
            <PartImages
              parts={item.images}
              className="max-h-96 max-w-full rounded-md border border-line/60"
              wrap="mt-2 flex flex-wrap gap-2"
            />
          )}
          {item.citations && item.citations.length > 0 && (
            <SourcesStrip citations={item.citations} />
          )}
          {!item.streaming && item.stopReason && item.stopReason !== "answered" && (
            <div className="mt-1 text-[12px] text-warn">
              stopped: {item.stopReason}
            </div>
          )}
          {!item.streaming && item.text.trim() && (
            <div className="mt-1 flex justify-end opacity-0 transition-opacity group-hover/msg:opacity-100">
              <CopyButton text={item.text} title="Copy message" />
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
      // A prompt from another door (the Telegram bridge, a sibling browser
      // window on the same session) renders as the user speaking through it;
      // a same-door prompt queued behind a busy turn keeps the queue
      // acknowledgment. Seam connections tag prompts "web:<n>" — the counter
      // is meaningless to a person, so label the door, not the connection.
      if (item.origin) {
        return (
          <div className="flex justify-end py-1">
            <div className="max-w-[85%] rounded-md border border-line/70 bg-card px-3 py-2">
              <div className="mb-0.5 text-[10.5px] uppercase tracking-wider text-faint select-none">
                via {item.origin.startsWith("web:") ? "another window" : item.origin}
              </div>
              <div className="whitespace-pre-wrap break-words text-bright">{item.text}</div>
              <PartImages
                parts={item.parts}
                wrap="mt-2 flex flex-wrap gap-2"
                className="max-h-48 max-w-full rounded-md border border-line/60 object-contain"
              />
            </div>
          </div>
        );
      }
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
});

/** The host of a citation URL, for a compact source label ("nytimes.com"). */
function citationHost(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, "");
  } catch {
    return url;
  }
}

/**
 * Web-search sources under an assistant message: a row of source chips the
 * provider grounded its answer on (OpenRouter's web_search annotations). The
 * search ran provider-side, so this is the only UI trace of it — there is no
 * tool card. Duplicate URLs collapse to one chip.
 */
function SourcesStrip({ citations }: { citations: Citation[] }) {
  const seen = new Set<string>();
  const unique = citations.filter((c) =>
    c.url && !seen.has(c.url) ? (seen.add(c.url), true) : false,
  );
  if (unique.length === 0) return null;
  return (
    <div className="mt-2 flex flex-wrap items-center gap-1.5">
      <Globe size={11} className="shrink-0 text-faint" />
      {unique.map((c, i) => (
        <a
          key={c.url}
          href={c.url}
          target="_blank"
          rel="noreferrer"
          title={c.title || c.url}
          className="max-w-[16rem] truncate rounded-full border border-line/60 bg-card/60 px-2 py-0.5 text-[11px] text-muted hover:text-bright"
        >
          <span className="text-faint">{i + 1}.</span> {c.title || citationHost(c.url)}
        </a>
      ))}
    </div>
  );
}

/** One reference line the composer spools per attached file (ChatTab's
 *  spoolAttachments — the formats must agree): name for the chip's label,
 *  workspace path for opening the spooled file in a panel. */
const ATTACH_LINE = /^Attached file "([^"]+)": `([^`]+)`$/;

interface AttachedFile {
  name: string;
  path: string;
}

/**
 * Pull the attachment reference lines out of a user message so the card can
 * render them as file chips instead of raw text. Parsing the persisted text
 * (rather than carrying structure) keeps replayed sessions identical to live
 * ones for free. Everything else stays as the typed body.
 */
function splitAttachments(text: string): { body: string; files: AttachedFile[] } {
  if (!text.includes("Attached file ")) return { body: text, files: [] };
  const files: AttachedFile[] = [];
  const rest: string[] = [];
  for (const line of text.split("\n")) {
    const m = ATTACH_LINE.exec(line);
    if (m) files.push({ name: m[1], path: m[2] });
    else rest.push(line);
  }
  return { body: rest.join("\n").trim(), files };
}

/** The chips themselves: filename rows that open the spooled file in a
 *  surface panel beside the chat (the same viewer show and diffs use). */
function AttachmentChips({
  files,
  onOpenSurface,
}: {
  files: AttachedFile[];
  onOpenSurface?: (surface: Surface) => void;
}) {
  if (files.length === 0) return null;
  return (
    <div
      className="flex flex-wrap gap-1.5"
      // The card's double-click opens edit mode; a fast double-click on a
      // chip should just open the file, not both.
      onDoubleClick={(e) => e.stopPropagation()}
    >
      {files.map((f) => (
        <button
          key={f.path}
          type="button"
          disabled={!onOpenSurface}
          onClick={() =>
            onOpenSurface?.({ ...fileSurface(f.path), title: f.name })
          }
          title={onOpenSurface ? `Open ${f.path}` : f.path}
          className={`flex items-center gap-1 rounded-md border border-line bg-canvas/60 px-2 py-0.5 text-[11.5px] text-muted transition-colors ${
            onOpenSurface ? "cursor-pointer hover:border-accent/50 hover:text-text" : ""
          }`}
        >
          <FileText size={11} className="shrink-0 text-faint" />
          <span className="max-w-[220px] truncate">{f.name}</span>
        </button>
      ))}
    </div>
  );
}

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
  onOpenSurface,
}: {
  item: Extract<TranscriptItem, { kind: "user" }>;
  edit?: TranscriptEditHooks;
  onOpenSurface?: (surface: Surface) => void;
}) {
  const editing = edit?.editingId === item.id;
  const { body, files } = splitAttachments(item.text);
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
          // Cursor's alignment: the card's TEXT shares the body prose column,
          // while the rounded background bleeds outward by its own padding (the
          // negative margin cancels the px), so a prompt and the answer beneath
          // it line up on the same left edge instead of the card text sitting
          // indented from the prose.
          className="group relative -mx-3 space-y-1.5 rounded-md border border-line/70 bg-card px-3 py-2 text-bright"
        >
          <AttachmentChips files={files} onOpenSurface={onOpenSurface} />
          {body && (
            <div className="whitespace-pre-wrap break-words">{body}</div>
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
    <div className="-mx-3 rounded-md border border-accent/50 bg-card px-3 py-2 text-bright ring-1 ring-accent/30">
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
  // The turn died (stop or error) before this call's result arrived: no
  // result is coming, so render a settled "stopped" row, never a spinner.
  if (item.interrupted && !item.result) return <StoppedToolRow item={item} />;
  // Still streaming its arguments in: the live composing card, until the
  // finished call lands and the row becomes its real (diff/terminal/…) card.
  if (item.composing && !item.result) return <ComposingRow item={item} />;
  switch (item.call.Name) {
    case "bash":
      return <TerminalCard item={item} hooks={hooks} />;
    case "edit":
    case "write":
      return <DiffCard item={item} hooks={hooks} />;
    case "plan":
      return <PlanItem item={item} />;
    case "delegate":
      return <DelegateChip item={item} hooks={hooks} />;
    case "show":
      return <ShowChip item={item} hooks={hooks} />;
    case "remember":
      return <MemoryCard item={item} />;
    case "ask":
      return <AskCard item={item} />;
    case "ls": {
      // The listed directory opens as a browser tab — same reference the
      // Files button and show-on-a-folder use.
      const path = str(args(item.call).path) || ".";
      const open =
        item.result && !item.result.IsError && hooks?.onOpenSurface
          ? () => hooks.onOpenSurface?.(dirSurface(path))
          : undefined;
      return <SummaryRow item={item} onArg={open} argTitle={`Browse ${path}`} />;
    }
    default:
      return <SummaryRow item={item} />;
  }
}

/** A present-tense label for a call whose arguments are still streaming in. */
function composingVerb(name: string): string {
  switch (name) {
    case "write":
      return "Writing";
    case "edit":
      return "Editing";
    default:
      return "Preparing";
  }
}

/** Bytes as a short human size — the streamed argument length, a stand-in for
 *  the artifact's size while it's still being composed. */
function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

/** What a cut-off call was doing, for its settled "stopped" row. */
function stoppedLabel(call: ToolCall): { verb: string; arg: string } {
  switch (call.Name) {
    case "bash":
      return { verb: "Running", arg: str(args(call).command) };
    case "write":
    case "edit":
      return { verb: composingVerb(call.Name), arg: basename(str(args(call).path)) };
    default:
      return summary(call);
  }
}

/**
 * A call the turn abandoned (interrupted mid-stream or mid-run): the quiet
 * settled row that replaces its spinner — what it was doing, then "stopped".
 */
function StoppedToolRow({ item }: { item: ToolTranscriptItem }) {
  const { verb, arg } = stoppedLabel(item.call);
  return (
    <div className="flex min-w-0 items-center gap-1.5 text-muted">
      <X size={12} className="shrink-0 text-faint" />
      <span className="shrink-0">{verb}</span>
      {arg && (
        <span className="truncate font-mono text-[11.5px] text-muted/80">{arg}</span>
      )}
      <span className="shrink-0 text-faint">— stopped</span>
    </div>
  );
}

/**
 * A tool call whose arguments are still streaming — the live "composing" card
 * that fills the gap a big call (a canvas's whole HTML body) would otherwise
 * leave silent: a spinner, a present-tense verb, and the bytes so far ticking
 * up. It becomes the call's real card (diff, terminal, …) the moment the
 * finished call lands.
 */
function ComposingRow({ item }: { item: ToolTranscriptItem }) {
  const bytes = item.composing?.bytes ?? 0;
  return (
    <div className="flex min-w-0 items-center gap-1.5 text-muted">
      <Loader2 size={12} className="shrink-0 animate-spin text-faint" />
      <span className="shrink-0 shimmer">{composingVerb(item.call.Name)}</span>
      {bytes > 0 && (
        <span className="shrink-0 font-mono text-[11px] text-faint">{fmtBytes(bytes)}</span>
      )}
    </div>
  );
}

/* ------------------------------------------------------------------ */
/* Ask: the question form in the transcript. While it runs the live    */
/* panel above the composer collects answers; this row is the durable  */
/* record — each prompt with the answer the user gave.                 */
/* ------------------------------------------------------------------ */

type AskQuestionArg = {
  id?: string;
  prompt?: string;
  options?: { id?: string; label?: string }[];
};

type AskAnswer = {
  question_id?: string;
  selected_ids?: string[];
  other_text?: string;
};

type AskDetails = {
  answers?: AskAnswer[];
  details?: string;
  skipped?: boolean;
};

function askDetails(item: ToolTranscriptItem): AskDetails {
  const d = item.result?.Details;
  return typeof d === "object" && d !== null ? (d as AskDetails) : {};
}

/** The labels the user chose for one question, "their own words" included. */
function askAnswerText(q: AskQuestionArg, answers: AskAnswer[]): string {
  const a = answers.find((x) => x.question_id === q.id);
  if (!a) return "";
  const labels = (a.selected_ids ?? []).map(
    (id) => q.options?.find((o) => o.id === id)?.label ?? id,
  );
  if (a.other_text) labels.push(a.other_text);
  return labels.join(", ");
}

function AskCard({ item }: { item: ToolTranscriptItem }) {
  const running = !item.result;
  const failed = item.result?.IsError ?? false;
  const a = args(item.call);
  const qs = Array.isArray(a.questions) ? (a.questions as AskQuestionArg[]) : [];
  const d = askDetails(item);

  return (
    <div className="rounded-md border border-line/60 bg-card/60 px-3 py-2">
      <div className="flex items-center gap-1.5">
        <CircleHelp size={12} className="shrink-0 text-muted" />
        <span className="text-[11px] font-medium uppercase tracking-wider text-faint">
          Questions
        </span>
        <span className="flex-1" />
        {running && (
          <Loader2 size={12} className="shrink-0 animate-spin text-faint" />
        )}
        {failed && <X size={12} className="shrink-0 text-red" />}
        {!running && !failed && d.skipped && (
          <span className="text-[11px] text-faint">skipped</span>
        )}
        {!running && !failed && !d.skipped && (
          <span className="flex items-center gap-1 text-[11px] text-faint">
            <Check size={12} className="shrink-0 text-green" />
            answered
          </span>
        )}
      </div>
      <div className="mt-1 space-y-1.5">
        {qs.map((q, i) => {
          const answer = askAnswerText(q, d.answers ?? []);
          return (
            <div key={q.id ?? i} className="min-w-0 text-[12.5px] leading-relaxed">
              <div className="break-words text-text">{q.prompt}</div>
              {answer && (
                <div className="break-words text-muted">↳ {answer}</div>
              )}
            </div>
          );
        })}
        {d.details && (
          <div className="break-words text-[12.5px] italic leading-relaxed text-muted">
            “{d.details}”
          </div>
        )}
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
      {/* The path earns its spot only when it says more than the label —
          "demo.md demo.md" told the user nothing twice. */}
      {(surface?.path ?? str(a.path)) !== label && (
        <span className="min-w-0 truncate font-mono text-[11px] text-faint">
          {surface?.path ?? str(a.path)}
        </span>
      )}
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

/** The TUI's braille spinner frames (internal/transcript/format.go). */
const BRAILLE_FRAMES = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const BRAILLE_TICK_MS = 80;

/** The animated braille spinner the TUI uses for in-flight activity. */
function BrailleSpinner() {
  const [frame, setFrame] = useState(0);
  useEffect(() => {
    const id = window.setInterval(
      () => setFrame((f) => (f + 1) % BRAILLE_FRAMES.length),
      BRAILLE_TICK_MS,
    );
    return () => window.clearInterval(id);
  }, []);
  return <>{BRAILLE_FRAMES[frame]}</>;
}

/** The row itself: `[spinner] title  backend` over a dim status line. */
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
        <span className="w-[13px] shrink-0 text-center font-mono text-[13px] leading-none text-muted">
          {running ? <BrailleSpinner /> : "⠿"}
        </span>
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

/** One dim line per quiet tool call, e.g. `Read web/src/App.tsx`. With
 * `onArg`, the argument is a door (ls rows open the browser there). */
function SummaryRow({
  item,
  onArg,
  argTitle,
}: {
  item: ToolTranscriptItem;
  onArg?: () => void;
  argTitle?: string;
}) {
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
        {arg && (onArg ? (
          <button
            type="button"
            onClick={onArg}
            title={argTitle}
            className="cursor-pointer truncate font-mono text-[11.5px] text-muted/80 hover:text-accent hover:underline"
          >
            {arg}
          </button>
        ) : (
          <span className="truncate font-mono text-[11.5px] text-muted/80">
            {arg}
          </span>
        ))}
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

/** How often a card's "running in background" badge re-checks the job. */
const JOB_BADGE_POLL_MS = 2000;

/**
 * A bash call as Cursor's terminal card: `[icon] Description command` in the
 * header (the description is the tool's display-only arg), dim output below.
 * Expandable into a terminal tab — every command is a journaled job, and the
 * job id rides the result's Details — where the full output tails live.
 */
function TerminalCard({ item, hooks }: { item: ToolTranscriptItem; hooks?: TranscriptHooks }) {
  const [open, setOpen] = useState(true);
  const [stopped, setStopped] = useState(false);
  const running = !item.result;
  const failed = item.result?.IsError ?? false;
  const a = args(item.call);
  const command = str(a.command) || argsPreview(item.call);
  const description = str(a.description);
  const { text: output, job } = trimJobReport(item.result?.Content.trimEnd() ?? "");
  const tail = output.split("\n").slice(-TERMINAL_TAIL_LINES).join("\n");

  // The job behind this card: Details when the result carries it, else the
  // backgrounded-report text (older transcripts). Errors carry no Details.
  const jobId = item.result ? (detailsJob(item.result) ?? job) : undefined;
  const expand =
    jobId && hooks?.onOpenTerminal
      ? (e: React.MouseEvent) => {
          e.stopPropagation();
          hooks.onOpenTerminal?.({ kind: "job", job: jobId, command });
        }
      : undefined;

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

  // The "running in background" report is a snapshot of the moment the tool
  // returned — the job usually outlives it. Poll the job's live status while
  // the badge claims it's running, so the spinner stops when the job does
  // instead of spinning forever over an exited process.
  const [done, setDone] = useState<{ status: string; exitCode: number } | null>(
    null,
  );
  const live = !!job && !stopped && !done;
  useEffect(() => {
    if (!job || !live) return;
    let stop = false;
    const check = () => {
      fetchJobTail(job, 0)
        .then((t) => {
          if (!stop && t.status !== "running") {
            setDone({ status: t.status, exitCode: t.exit_code });
          }
        })
        .catch((e: unknown) => {
          // A job the gateway no longer knows can't still be running; a
          // network blip keeps polling.
          if (!stop && e instanceof HttpError && e.status === 404) {
            setDone({ status: "ended", exitCode: 0 });
          }
        });
    };
    check();
    const id = window.setInterval(check, JOB_BADGE_POLL_MS);
    return () => {
      stop = true;
      window.clearInterval(id);
    };
  }, [job, live]);

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
        {expand && (
          <span
            onClick={expand}
            title="Open as terminal tab"
            className="flex size-5 shrink-0 cursor-pointer items-center justify-center rounded text-faint transition-colors hover:bg-hover hover:text-text"
          >
            <Maximize2 size={11} />
          </span>
        )}
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
              ) : done ? (
                done.status === "exited" ? (
                  <>
                    <Check size={11} />
                    finished · exit {done.exitCode} · {job}
                  </>
                ) : (
                  <>
                    <X size={11} />
                    {done.status} · {job}
                  </>
                )
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

/** An edit/write call as Cursor's diff card: filename + counts, tinted lines.
 * The filename opens the file in a panel beside the chat (like Cursor's
 * click-through from a diff to the editor); the rest of the header toggles
 * the diff. */
function DiffCard({ item, hooks }: { item: ToolTranscriptItem; hooks?: TranscriptHooks }) {
  const [open, setOpen] = useState(true);
  const running = !item.result;
  const failed = item.result?.IsError ?? false;
  const path = str(args(item.call).path);
  const name = basename(path);
  const lines = diffLines(item);
  const adds = lines.filter((l) => l.kind === "add").length;
  const dels = lines.filter((l) => l.kind === "del").length;

  const openFile =
    path && hooks?.onOpenSurface
      ? (e: React.MouseEvent) => {
          e.stopPropagation();
          hooks.onOpenSurface?.(fileSurface(path));
        }
      : undefined;

  return (
    <div className="overflow-hidden rounded-md border border-line/80">
      <button
        type="button"
        onClick={() => setOpen(!open)}
        className="flex w-full cursor-pointer items-center gap-2 bg-card px-3 py-1.5 text-left"
      >
        <span className="size-1.5 shrink-0 rounded-full bg-accent" />
        <span
          onClick={openFile}
          title={openFile ? `Open ${path}` : undefined}
          className={`min-w-0 truncate text-[12px] text-text ${
            openFile ? "hover:text-accent hover:underline" : ""
          }`}
        >
          {name}
        </span>
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

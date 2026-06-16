import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from "react";
import {
  ArrowUp,
  FileText,
  Loader2,
  Mic,
  Paperclip,
  Square,
  X,
} from "lucide-react";

import { ApprovalCard } from "./ApprovalCard";
import { BackgroundBar } from "./BackgroundBar";
import { QuestionCard, type QuestionCardHandle } from "./QuestionCard";
import { ChatView } from "./ChatView";
import { CommandMenu } from "./CommandMenu";
import { ContextCircle } from "./ContextCircle";
import { ModelPicker } from "./ModelPicker";
import { QueuedMessages, type QueuedMessage } from "./QueuedMessages";
import type { RunRef } from "./RunView";
import { Tooltip } from "./Tooltip";
import {
  cancelPlanNode,
  fetchChildren,
  fetchJobTail,
  fetchModels,
  fetchReplay,
  fetchReplayFull,
  HttpError,
  killJob,
  startVoice,
  stopRun,
  stopVoice,
  transcribeAudio,
  writeFile,
  writeFileBase64,
  type ChildRun,
  type ModelOption,
} from "@/lib/api";
import { notifyRunComplete } from "@/lib/notify";
import {
  hostName,
  isSharedSession,
  onHostNameChange,
  onSharedChange,
} from "@/lib/identity";
import {
  blobBase64,
  cancelRecording,
  recorderSupported,
  startRecording,
  stopRecording,
} from "@/lib/recorder";
import { useDocumentVisible } from "@/lib/useDocumentVisible";
import { useSlashCommands } from "@/lib/useSlashCommands";
import { SeamClient, type ConnectionState } from "@/lib/seam";
import {
  detailsSurface,
  detailsUI,
  peopleSurface,
  promptSurface,
  type Surface,
  type UICommand,
} from "@/lib/surface";
import type { TermRef } from "@/lib/term";
import type { ContentBlock, QuestionAnswer, ToolCall, ToolResult } from "@/lib/types";
import { useAutosize } from "@/lib/useAutosize";
import {
  chatReducer,
  initialChatState,
  replayToItems,
  type TranscriptItem,
} from "@/lib/transcript";

/** Socket reconnect: exponential backoff with jitter, so a restarting
 *  backend isn't hammered and a fleet of tabs doesn't thunder in sync. */
const RECONNECT_BASE_MS = 1000;
const RECONNECT_MAX_MS = 15000;
/** Re-bind while the server still holds the session for a dead connection:
 *  it frees the moment the server notices, so retry forever (with backoff) —
 *  never silently fall back to a fresh session under the old transcript. */
const REBIND_BASE_MS = 1000;
const REBIND_MAX_MS = 8000;
/** How long a connection must stay down before the reconnecting banner
 *  shows. Sub-grace drops (idle reaping, blips) heal invisibly. */
const RECONNECT_BANNER_DELAY_MS = 3000;

/** A retry delay with jitter: 50–100% of the capped exponential step. */
function backoffMs(attempt: number, base: number, max: number): number {
  const exp = Math.min(max, base * 2 ** Math.min(attempt, 10));
  return exp / 2 + Math.random() * (exp / 2);
}

/** Model id from a successful set_model tool call — nil when rejected or failed. */
function modelFromSetModelTool(call: ToolCall, result: ToolResult): string | null {
  if (call.Name !== "set_model" || result.IsError) return null;
  if (result.Content.startsWith("Did not switch")) return null;
  const args =
    typeof call.Args === "object" && call.Args !== null
      ? (call.Args as Record<string, unknown>)
      : {};
  const model = typeof args.model === "string" ? args.model.trim() : "";
  return model || null;
}
const COMPOSER_MAX_PX = 200;
const RUNS_POLL_MS = 5000;
// An edit mid-turn: the interrupt needs a beat to wind the engine down before
// the fork lands; retry on "fork: busy" until it does (or give up).
const FORK_RETRY_MS = 250;
const FORK_RETRY_MAX = 8;
/** Cap per attached file so a spool never PUTs an unbounded blob (matches the
 *  gateway's 2 MiB /api/file read cap — anything bigger truncates anyway). */
const ATTACH_MAX_CHARS = 2_000_000;
/** Where attached files spool in the workspace (gitignored). */
const ATTACH_DIR = ".arbos/attachments";

/** Cap a binary attachment (PDF) so its base64 stays under the seam's
 *  WebSocket frame limit (32 MiB; base64 inflates ~33%). */
const ATTACH_MAX_BYTES = 24 * 1024 * 1024;

/**
 * A file picked into the composer. Text files spool to the workspace on send
 * and the prompt carries a one-line path reference (the agent reads the file
 * with its own tools; the transcript renders the line as an openable chip).
 * Images ride along as base64 multimodal parts the seam carries to a vision
 * model (ADR-0022). PDFs (kind "file") do both: they spool to disk (so the
 * chip opens the document in a panel) AND ride along as a file part the
 * provider parses natively (text + OCR for scanned pages).
 */
interface Attachment {
  aid: number;
  name: string;
  kind: "text" | "image" | "file";
  text?: string; // kind "text": file contents
  data?: string; // kind "image"/"file": base64 payload (no data: prefix)
  mime?: string; // kind "image"/"file": MIME type
  /** A voice memo still transcribing: the chip shows a spinner and submit
   *  waits for the transcript to land before spooling. */
  pending?: boolean;
}

/** Audio container formats the transcription endpoint accepts, by extension. */
const AUDIO_FORMATS = new Set(["wav", "mp3", "flac", "m4a", "ogg", "webm", "aac"]);

/** The transcription format label for an audio file, or "" if not audio. */
function audioFormat(file: File): string {
  const ext = file.name.toLowerCase().split(".").pop() ?? "";
  if (AUDIO_FORMATS.has(ext)) return ext;
  // Opus voice memos (WhatsApp .opus, Telegram .oga) are Ogg containers.
  if (ext === "opus" || ext === "oga") return "ogg";
  if (file.type.startsWith("audio/")) {
    const sub = file.type.slice("audio/".length).split(";")[0];
    if (sub === "mpeg") return "mp3";
    if (sub === "mp4" || sub === "x-m4a") return "m4a";
    if (sub === "opus") return "ogg";
    if (AUDIO_FORMATS.has(sub)) return sub;
  }
  return "";
}

function errMsg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** A spooled file's destination: a short unique prefix keeps re-attaches of
 *  the same name apart while the original name stays readable in the path. */
function attachmentPath(name: string): string {
  const safe = name.replace(/[^\w.-]+/g, "_");
  return `${ATTACH_DIR}/${Date.now().toString(36)}-${safe}`;
}

/**
 * Spool the composer's text attachments to the workspace through the
 * gateway's file door and return one reference line per file. The line is
 * the whole contract: the model reads the path with its own tools, and the
 * transcript parses it back into a chip that opens the file in a panel
 * (ChatView's splitAttachments) — which is why its shape must not drift.
 */
async function spoolAttachments(attachments: Attachment[]): Promise<string[]> {
  return Promise.all(
    attachments
      .filter((a) => a.kind === "text" || a.kind === "file")
      .map(async (a) => {
        const saved =
          a.kind === "file"
            ? await writeFileBase64(attachmentPath(a.name), a.data ?? "")
            : await writeFile(attachmentPath(a.name), a.text ?? "");
        return `Attached file "${a.name}": \`${saved.path}\``;
      }),
  );
}

/**
 * Compose the prompt the seam carries: the attachments' reference lines, then
 * the typed message. Images are not folded — they go as parts. Empty pieces
 * drop out, so an image-only send still works.
 *
 * A slash command must stay at position 0 — the engine's template expansion
 * keys on the leading "/" — so for `/cmd …` the typed text leads and the
 * attachments follow (folding into the command's arguments).
 */
function composeMessage(text: string, fileLines: string[]): string {
  const typed = text.trim();
  const pieces = typed.startsWith("/") ? [typed, ...fileLines] : [...fileLines, typed];
  return pieces.filter(Boolean).join("\n\n");
}

/** The image and document attachments as multimodal content blocks for the
 *  prompt: images as image blocks, PDFs as file blocks the provider parses. */
function mediaParts(attachments: Attachment[]): ContentBlock[] {
  return attachments
    .filter((a): a is Attachment & { data: string } => a.kind !== "text" && !!a.data)
    .map((a) =>
      a.kind === "file"
        ? {
            type: "file",
            file: { data: a.data, mimeType: a.mime ?? "application/pdf", name: a.name },
          }
        : { type: "image", image: { data: a.data, mimeType: a.mime ?? "image/png" } },
    );
}

export interface ChatTabHandle {
  /** Live activity the tab strip renders (spinner on busy tabs). */
  onBusy?: (busy: boolean) => void;
  /** First prompt of a fresh tab — becomes the tab title. */
  onTitle?: (title: string) => void;
  /** The session id this tab is bound to (assigned or resumed). */
  onSession?: (id: string) => void;
  /** The agent (or a chip click) opened a surface — show it beside the chat. */
  onOpenSurface?: (surface: Surface) => void;
  /** The agent issued a UI command (the `ui` tool) — close/focus side panels. */
  onControlUI?: (cmd: UICommand) => void;
  /** A sub-agent run was clicked — open its transcript in a tab beside the
   *  chat, where a long-running run stays watchable. */
  onOpenRun?: (run: RunRef) => void;
  /** A terminal card (or background-bar job) was expanded — open it as a
   *  terminal tab tailing the job's live journal. */
  onOpenTerminal?: (term: TermRef) => void;
  /** A plan card was clicked — open its goal tree and code in a panel. */
  onOpenPlan?: (node: number) => void;
}

/**
 * One agent session: its own seam connection, transcript, and composer.
 * Enter sends; while a turn runs Enter steers (replaces the in-flight turn)
 * and Esc interrupts. Approvals answer with a click or y / n.
 *
 * A tab given `resumeId` seeds its transcript from the session's persisted
 * history, then binds the live seam to the same id.
 *
 * `active` means visible in its pane; with a split two tabs are active at
 * once, so the singular concerns (ambient notices, owning the keyboard) key
 * off `focused` — true for exactly one tab, the focused pane's visible one.
 */
export function ChatTab({
  active,
  focused,
  focusTick,
  resumeId,
  handle,
  readOnly,
  sharedTab,
}: {
  active: boolean;
  focused: boolean;
  /** Bumped by explicit activation (tab click, new tab, split): grab focus. */
  focusTick: number;
  resumeId: string | null;
  handle: ChatTabHandle;
  /** Read-only view (a read-scoped share): render the transcript, but no
   *  composer or interactive cards — the viewer can watch, not drive. */
  readOnly?: boolean;
  /** This tab is a shared session (a guest arrived via a share link): open the
   *  People panel by default. The host's tab opens it once it shares. */
  sharedTab?: boolean;
}) {
  const [chat, dispatch] = useReducer(chatReducer, initialChatState);
  const [connState, setConnState] = useState<ConnectionState>("idle");
  const [text, setText] = useState("");
  // The chat's scheduled runs (plan node firings, from the gateway), for the
  // background bar's rows; clicking one opens it in a run tab beside us.
  const [runs, setRuns] = useState<ChildRun[]>([]);
  const [boundSession, setBoundSession] = useState<string | null>(resumeId);
  // The host's display name for this chat (set in the Share dialog). Kept in
  // state so setting it relabels the host's own messages live.
  const [selfName, setSelfName] = useState(() => hostName(resumeId));
  // Mirror selfName into a ref so the long-lived seam handlers (built once) read
  // the current value rather than the value captured at construction.
  const selfNameRef = useRef(selfName);
  selfNameRef.current = selfName;
  // People side chat: a human-to-human board for collaborators, separate from
  // the agent transcript and never seen by the model. It lives entirely in its
  // own panel (the "people" surface) — a normal tab opened from the top row,
  // splittable like any other — which owns the roster, message list, presence,
  // and sends on its own scoped seam. The chat tab holds no People UI of its
  // own.
  // Messages composed while a turn runs wait here (Cursor's queue): each can
  // be edited back into the composer, deleted, or pushed (sent now as a
  // steer). The head auto-sends when the turn completes.
  const [queue, setQueue] = useState<QueuedMessage[]>([]);
  const qidRef = useRef(1);
  // A past user message opened for inline editing (rewind-and-resubmit).
  const [editingId, setEditingId] = useState<number | null>(null);
  // An edit whose fork was requested but not yet confirmed: the truncated
  // transcript and replacement prompt to apply when the forked frame lands.
  // Kept out of the transcript until then so a failed fork changes nothing.
  // An edit mid-turn rides this too: the server refuses to fork under an
  // in-flight turn, so the busy error interrupts it and retries the fork.
  const pendingEditRef = useRef<{
    throughSeq: number;
    keep: TranscriptItem[];
    text: string;
    parts: ContentBlock[];
    retries: number;
  } | null>(null);
  // Files picked into the composer (paperclip): text files spool to the
  // workspace on send (the prompt carries path references) and the chips
  // clear once it's sent.
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  // Latest attachments for submit's post-await read (transcripts resolve
  // after the closure captured state).
  const attachmentsRef = useRef(attachments);
  attachmentsRef.current = attachments;
  const aidRef = useRef(1);
  // The active model, shown in the composer's picker. Seeds from the host's
  // configured default; a pick sends set_model on the live seam and re-labels.
  const [model, setModel] = useState("");
  // The provider catalog, kept so the composer can size the context gauge
  // against the active model's context_length.
  const [catalog, setCatalog] = useState<ModelOption[]>([]);
  // Dictation (the mic button): the host machine captures its own microphone
  // and transcribes on-device. "starting"/"transcribing" are the host's
  // round-trips; "recording" is live capture, toggled off with another click.
  const [voice, setVoice] = useState<"idle" | "starting" | "recording" | "transcribing">(
    "idle",
  );

  // Socket open but the session is still held by a dead predecessor
  // connection: the tab is re-binding, not usable yet. Drives the
  // reconnecting banner and keeps the composer honest.
  const [rebinding, setRebinding] = useState(false);
  // The seam has been open at least once — so a later non-open state is a
  // LOST connection (show the banner), not the initial handshake.
  const [hadConn, setHadConn] = useState(false);

  const seamRef = useRef<SeamClient | null>(null);
  const sessionRef = useRef<string | null>(resumeId);
  const rootRef = useRef<HTMLDivElement>(null);
  const taRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const handleRef = useRef(handle);
  handleRef.current = handle;
  const focusedRef = useRef(focused);
  focusedRef.current = focused;
  // Fresh turn state for async callbacks (resubmitEdit reads it post-await).
  const turnActiveRef = useRef(chat.turnActive);
  turnActiveRef.current = chat.turnActive;
  // Whole chat state for the reconnect reconciler (it must see pending
  // approval/question panels, which a blind replay would wipe).
  const chatRef = useRef(chat);
  chatRef.current = chat;

  useEffect(() => {
    let retry: number | undefined;
    let forkRetry: number | undefined;
    let connectAttempts = 0;
    let rebinds = 0;
    // The socket dropped while a session was bound: deltas may have been
    // missed, so the next successful re-bind reconciles from the persisted
    // log instead of trusting the on-screen transcript.
    let gap = false;
    // A gap landed mid-turn: heal the transcript from the log once the turn
    // reaches a terminal event (replaying earlier would drop a pending
    // approval/question panel, and the in-flight prose isn't persisted yet).
    let healAtTurnEnd = false;
    let closed = false;

    /** Replace the transcript with the persisted session log — the source of
     *  truth after a connection gap. Failure leaves the screen as-is; the
     *  next heal (or reload) retries. */
    const reconcile = () => {
      const sid = sessionRef.current;
      if (!sid) return;
      fetchReplay(sid)
        .then((events) => {
          if (!closed && sessionRef.current === sid) {
            dispatch({ type: "replay", items: replayToItems(events) });
          }
        })
        .catch(() => {});
    };

    const reconcileAfterGap = () => {
      const c = chatRef.current;
      if (!c.turnActive) {
        reconcile();
        return;
      }
      healAtTurnEnd = true;
      // Mid-turn with no panel parked on the user: reconcile now too, so a
      // turn that actually FINISHED during the gap (its terminal event was
      // missed) doesn't leave the tab stuck "working" forever.
      if (!c.pendingApproval && !c.pendingQuestions) reconcile();
    };

    const seam = new SeamClient({
      onState: (s) => {
        setConnState(s);
        if (s === "open") {
          setHadConn(true);
          connectAttempts = 0;
          rebinds = 0;
          // First connect opens fresh (or resumes by id); a reconnect
          // re-binds the same session, so a dropped socket never loses the
          // thread.
          seam.open(sessionRef.current ?? undefined);
        }
        if (s === "closed") {
          // Any fork awaiting confirmation died with the socket; the rebind
          // below reattaches the original session, transcript intact.
          pendingEditRef.current = null;
          if (sessionRef.current) gap = true;
          if (!closed) {
            retry = window.setTimeout(
              () => seam.connect(),
              backoffMs(connectAttempts++, RECONNECT_BASE_MS, RECONNECT_MAX_MS),
            );
          }
        }
      },
      onSession: (id) => {
        rebinds = 0;
        setRebinding(false);
        const rebound = gap && id === sessionRef.current;
        gap = false;
        sessionRef.current = id;
        setBoundSession(id);
        // Announce our display name so the roster names us (host name is
        // client-side; a guest's announce is ignored server-side). Re-fires on
        // every rebind, so a reconnect re-registers us automatically.
        seam.announceName(hostName(id));
        handleRef.current.onSession?.(id);
        if (rebound) reconcileAfterGap();
      },
      onForked: () => {
        // The server confirmed a rewind-and-edit fork and rebound this
        // connection to the branch — now it is safe to truncate the visible
        // transcript and restart the thread from the edited message.
        const edit = pendingEditRef.current;
        if (!edit) return;
        pendingEditRef.current = null;
        dispatch({ type: "replay", items: edit.keep });
        if (seam.prompt(edit.text, edit.parts, hostName(sessionRef.current))) {
          dispatch({ type: "user", text: edit.text, parts: edit.parts });
        }
      },
      onEnvelope: (env) => {
        // A human-to-human side-chat line belongs to the People panel, not the
        // agent transcript: the People surface owns the message list on its own
        // scoped seam, so the chat tab simply ignores chat_notes here. depth>0
        // (a delegated child) never surfaces here.
        if (env.depth === 0 && env.event.kind === "chat_note") {
          return;
        }
        // The agent presenting a file (the show tool) opens its panel the
        // moment the result streams in — live only, top-level only: a
        // resumed transcript renders chips to re-open, and a delegated
        // child's shows stay quiet until clicked.
        if (env.depth === 0 && env.event.kind === "tool_finished") {
          const result = env.event.data.result;
          const surface = detailsSurface(result);
          if (surface) handleRef.current.onOpenSurface?.(surface);
          const ui = detailsUI(result);
          if (ui) handleRef.current.onControlUI?.(ui);
          // The agent's set_model tool stages a switch the engine applies at
          // the turn boundary — mirror a manual picker change so the chip
          // doesn't keep showing the old model.
          for (let i = chatRef.current.items.length - 1; i >= 0; i--) {
            const it = chatRef.current.items[i];
            if (it.kind === "tool" && it.call.ID === result.CallID) {
              const m = modelFromSetModelTool(it.call, result);
              if (m) setModel(m);
              break;
            }
          }
        }
        dispatch({ type: "envelope", env });
        // A gap landed mid-turn: now that the turn has settled, the persisted
        // log is whole — heal the transcript from it.
        if (
          healAtTurnEnd &&
          env.depth === 0 &&
          (env.event.kind === "turn_complete" ||
            env.event.kind === "interrupted" ||
            env.event.kind === "error")
        ) {
          healAtTurnEnd = false;
          reconcile();
        }
      },
      onNotice: (text, session) => {
        // A notice addressed to THIS chat always renders here, focused or
        // not — it was claimed for this tab and exists nowhere else. Ambient
        // ones (broadcast-class, which belong to no chat) fan out to every
        // connection; only the focused tab renders those (with a split two
        // tabs are visible, exactly one is focused), so the user sees them
        // exactly once.
        if (session === sessionRef.current || focusedRef.current) {
          dispatch({ type: "notice", text });
        }
      },
      onError: (msg) => {
        // An edit landed while a turn was in flight: the server refuses to
        // fork under it. Interrupt the turn (a no-op once it has ended) and
        // retry the fork until the engine has wound down.
        if (pendingEditRef.current && msg.startsWith("fork: busy")) {
          const edit = pendingEditRef.current;
          if (edit.retries++ < FORK_RETRY_MAX) {
            seam.interrupt();
            forkRetry = window.setTimeout(() => {
              if (pendingEditRef.current === edit) seam.fork(edit.throughSeq);
            }, FORK_RETRY_MS);
            return;
          }
          pendingEditRef.current = null;
          dispatch({ type: "seam-error", message: `edit: ${msg}` });
          return;
        }
        // Any other failed fork leaves the server bound to the original
        // session and the transcript untouched; just drop the waiting edit.
        if (pendingEditRef.current && msg.startsWith("fork:")) {
          pendingEditRef.current = null;
        }
        // A reconnect can land while the server still drains the previous
        // connection's hold on the session. It frees the moment that
        // teardown lands, so keep re-binding with backoff — NEVER fall back
        // to a fresh session, which would silently orphan the transcript on
        // screen. The banner shows "reconnecting" until the bind succeeds.
        if (msg.includes("already active") && sessionRef.current) {
          setRebinding(true);
          retry = window.setTimeout(
            () => seam.open(sessionRef.current ?? undefined),
            backoffMs(rebinds++, REBIND_BASE_MS, REBIND_MAX_MS),
          );
          return;
        }
        dispatch({ type: "seam-error", message: msg });
      },
    });
    seamRef.current = seam;

    if (resumeId) {
      // Paint history before the live connection lands, and seed the
      // composer's controls from the session record — a resumed session
      // keeps its model (durable, server-side), so the chip must show that
      // state, not the host defaults.
      fetchReplayFull(resumeId)
        .then(({ events, session }) => {
          dispatch({ type: "replay", items: replayToItems(events) });
          if (session?.model) setModel(session.model);
        })
        .catch(() => {})
        .finally(() => seam.connect());
    } else {
      seam.connect();
    }

    return () => {
      closed = true;
      window.clearTimeout(retry);
      window.clearTimeout(forkRetry);
      seam.close();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Busy state feeds the tab spinner; a true→false transition is "the agent
  // finished responding" — the moment the completion sound / spoken-response /
  // notification settings hook into. The latest transcript rides a ref so the
  // effect keys only on the transition, not on every streaming delta.
  const wasTurnActive = useRef(false);
  const itemsRef = useRef(chat.items);
  itemsRef.current = chat.items;
  useEffect(() => {
    handleRef.current.onBusy?.(chat.turnActive);
    if (wasTurnActive.current && !chat.turnActive) {
      const items = itemsRef.current;
      let finalText = "";
      for (let i = items.length - 1; i >= 0; i--) {
        const it = items[i];
        if (it.kind === "assistant") {
          finalText = it.text;
          break;
        }
        // An interrupted or errored turn has no reply worth speaking — the
        // nearest assistant text behind those markers is a cut-off partial.
        if (it.kind === "user" || it.kind === "interrupted" || it.kind === "error") break;
      }
      notifyRunComplete(null, finalText);
    }
    wasTurnActive.current = chat.turnActive;
  }, [chat.turnActive]);

  // The page is the prompt: clicking anywhere in this pane focuses its
  // composer (scoped by containment — with a split each pane focuses its
  // own). But a click landing on its own focusable control (a popup's search
  // box, another input) must keep that focus — otherwise the composer steals
  // every keystroke. `[data-keep-focus]` lets a popup opt its subtree out.
  useEffect(() => {
    if (!active) return;
    const focus = (e: MouseEvent) => {
      if (window.getSelection()?.toString()) return; // don't steal a selection
      const target = e.target as HTMLElement | null;
      if (!target || !rootRef.current?.contains(target)) return;
      if (
        target.closest(
          'input, textarea, select, [contenteditable="true"], [data-keep-focus]',
        )
      ) {
        return;
      }
      taRef.current?.focus();
    };
    document.addEventListener("click", focus);
    return () => document.removeEventListener("click", focus);
  }, [active]);

  // Explicit activation — a tab click, a new tab, a split — hands the
  // keyboard to the focused pane's composer. Plain clicks inside a pane are
  // handled above; pane focus shifting under a click on an inner input must
  // NOT re-grab (the user aimed at that input), hence the tick, not `focused`.
  useEffect(() => {
    if (active && focusedRef.current) taRef.current?.focus();
  }, [active, focusTick]);

  // Autosize the composer to its content, capped so it never eats the page.
  // `active` is the remeasure key: a hidden tab measures scrollHeight as 0, so
  // re-running on show keeps the empty composer from collapsing and letting the
  // placeholder overlap the model picker after a tab switch.
  useAutosize(taRef, text, COMPOSER_MAX_PX, active);

  // Poll this chat's scheduled runs (plan-node agent firings) so they appear
  // as openable tabs. Scoped server-side: only runs whose node THIS chat
  // created come back, so nothing leaks across conversations. Pauses while
  // the window is hidden; the visibility flip re-runs it immediately.
  const docVisible = useDocumentVisible();
  useEffect(() => {
    if (!active || !docVisible || !boundSession) return;
    let stop = false;
    const tick = () => {
      fetchChildren(boundSession)
        .then((kids) => {
          if (!stop) setRuns(kids);
        })
        .catch(() => {});
    };
    tick();
    const id = window.setInterval(tick, RUNS_POLL_MS);
    return () => {
      stop = true;
      window.clearInterval(id);
    };
  }, [active, docVisible, boundSession]);

  // The background bar's jobs list is folded from the transcript, which only
  // learns of a job's end if some LATER tool result happens to mention it —
  // a turn that ends with the job still running would pin "1 background
  // terminal" forever. Poll the live status of each listed job and drop the
  // ones that exited, so the bar agrees with the cards (and reality).
  const jobIDs = useMemo(
    () => chat.jobs.map((j) => j.id).join(","),
    [chat.jobs],
  );
  useEffect(() => {
    if (!active || !docVisible || !jobIDs) return;
    let stop = false;
    const tick = () => {
      for (const id of jobIDs.split(",")) {
        fetchJobTail(id, 0)
          .then((t) => {
            if (!stop && t.status !== "running") {
              dispatch({ type: "job-removed", id });
            }
          })
          .catch((e: unknown) => {
            // Unknown to the gateway = certainly not running.
            if (!stop && e instanceof HttpError && e.status === 404) {
              dispatch({ type: "job-removed", id });
            }
          });
      }
    };
    tick();
    const timer = window.setInterval(tick, RUNS_POLL_MS);
    return () => {
      stop = true;
      window.clearInterval(timer);
    };
  }, [active, docVisible, jobIDs]);

  const connected = connState === "open";
  // Usable = socket open AND a session bound; while re-binding the seam is
  // open but frames addressed to the session would only error.
  const usable = connected && !rebinding;
  // The thread was live and now isn't. Most drops heal in well under the
  // grace window (idle reaping by a proxy, a network blip), so the banner
  // waits it out — flashing "connection lost" for a self-healing hiccup
  // reads as the app being broken when nothing was lost.
  const down = rebinding || (hadConn && !connected);
  const [reconnecting, setReconnecting] = useState(false);
  useEffect(() => {
    if (!down) {
      setReconnecting(false);
      return;
    }
    const t = window.setTimeout(() => setReconnecting(true), RECONNECT_BANNER_DELAY_MS);
    return () => window.clearTimeout(t);
  }, [down]);
  const approval = chat.pendingApproval;
  const questions = chat.pendingQuestions;
  const questionRef = useRef<QuestionCardHandle>(null);

  // The "working" heartbeat (ChatView's shimmer line): live the instant a
  // turn starts, so a sent prompt never sits in silence. ChatView suppresses
  // it while something is already streaming; here we also hush it while the
  // turn is parked on the user for an approval or a question.
  const working = chat.turnActive && !approval && !questions;

  // Voice memos still transcribing when the user hits Enter: submit awaits
  // these so a fast send never spools an empty transcript.
  const transcribingRef = useRef(new Map<number, Promise<void>>());

  const onPickFiles = useCallback((files: FileList | null) => {
    if (!files) return;
    for (const file of Array.from(files)) {
      const isPdf =
        file.type === "application/pdf" || file.name.toLowerCase().endsWith(".pdf");
      const audioFmt = audioFormat(file);
      if (audioFmt) {
        // A voice memo: transcribe it through the provider's audio endpoint
        // and attach the transcript as text — every model can read text, so
        // the memo works regardless of which model is driving the session.
        if (file.size > ATTACH_MAX_BYTES) {
          dispatch({
            type: "seam-error",
            message: `attach: "${file.name}" is too large (max ${Math.round(ATTACH_MAX_BYTES / 1024 / 1024)} MB)`,
          });
          continue;
        }
        const aid = aidRef.current++;
        const name = `${file.name}.txt`;
        setAttachments((a) => [...a, { aid, name, kind: "text", pending: true }]);
        const job = (async () => {
          const heard = await transcribeAudio(await blobBase64(file), audioFmt);
          setAttachments((a) =>
            a.map((x) =>
              x.aid === aid
                ? {
                    ...x,
                    pending: false,
                    text: `Voice memo "${file.name}" (transcribed):\n\n${heard}`,
                  }
                : x,
            ),
          );
        })();
        transcribingRef.current.set(
          aid,
          job.catch((e: unknown) => {
            setAttachments((a) => a.filter((x) => x.aid !== aid));
            dispatch({ type: "seam-error", message: `attach: ${errMsg(e)}` });
          }).finally(() => transcribingRef.current.delete(aid)),
        );
        continue;
      }
      if (file.type.startsWith("image/") || isPdf) {
        // Images and PDFs go to the model as base64 multimodal parts; a PDF
        // also spools to disk so its chip opens the document in a panel.
        if (isPdf && file.size > ATTACH_MAX_BYTES) {
          dispatch({
            type: "seam-error",
            message: `attach: "${file.name}" is too large (max ${Math.round(ATTACH_MAX_BYTES / 1024 / 1024)} MB)`,
          });
          continue;
        }
        const reader = new FileReader();
        reader.onload = () => {
          const url = String(reader.result);
          const data = url.slice(url.indexOf(",") + 1);
          setAttachments((a) => [
            ...a,
            {
              aid: aidRef.current++,
              name: file.name,
              kind: isPdf ? "file" : "image",
              data,
              mime: file.type || (isPdf ? "application/pdf" : "image/png"),
            },
          ]);
        };
        reader.readAsDataURL(file);
      } else {
        // Everything else is read as text now and spooled to disk on send.
        file
          .text()
          .then((content) =>
            setAttachments((a) => [
              ...a,
              {
                aid: aidRef.current++,
                name: file.name,
                kind: "text",
                text: content.slice(0, ATTACH_MAX_CHARS),
              },
            ]),
          )
          .catch(() => {});
      }
    }
  }, [dispatch]);

  // Paste an image (or file) straight from the clipboard — a screenshot or a
  // copied image rides the same path as the picker. A plain-text paste carries
  // no files, so it falls through to the textarea's default handling.
  const onPaste = useCallback(
    (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
      const files = e.clipboardData?.files;
      if (files && files.length > 0) {
        e.preventDefault();
        onPickFiles(files);
      }
    },
    [onPickFiles],
  );

  // Seed the picker with the host's configured model so the chip names the
  // model running before the user touches it.
  useEffect(() => {
    fetchModels()
      .then((c) => {
        setModel((m) => m || c.current);
        setCatalog(c.models);
      })
      .catch(() => {});
  }, []);

  // The context gauge: how full the active model's window is, from the latest
  // turn's prompt+completion tokens against its catalog context_length.
  const contextLength = useMemo(
    () => catalog.find((m) => m.id === model)?.context_length ?? 0,
    [catalog, model],
  );
  const usedTokens = useMemo(() => {
    for (let i = chat.items.length - 1; i >= 0; i--) {
      const it = chat.items[i];
      if (it.kind === "assistant" && it.usage) {
        return it.usage.PromptTokens + it.usage.CompletionTokens;
      }
    }
    return 0;
  }, [chat.items]);

  // The engine applies set_model only between turns (idle-only contract); a
  // frame sent mid-turn is dropped server-side while the chip has already
  // flipped. The contract says the frontend resends once idle — this dirty
  // flag plus the effect below is that resend, so a mid-turn click still
  // means what it shows.
  const controlsDirtyRef = useRef(false);
  const controlsRef = useRef({ model: "" });
  controlsRef.current = { model };
  const markDirtyIfBusy = useCallback(() => {
    if (turnActiveRef.current) controlsDirtyRef.current = true;
  }, []);
  useEffect(() => {
    if (chat.turnActive || !controlsDirtyRef.current) return;
    controlsDirtyRef.current = false;
    const seam = seamRef.current;
    const c = controlsRef.current;
    if (!seam) return;
    if (c.model) seam.setModel(c.model);
  }, [chat.turnActive]);

  // Switch the model on the live session; only re-label once the seam accepts
  // the frame, so a dropped connection doesn't lie about what's running.
  const selectModel = useCallback((id: string) => {
    if (seamRef.current?.setModel(id)) {
      setModel(id);
      markDirtyIfBusy();
    }
  }, [markDirtyIfBusy]);

  // Toggle dictation: first click starts capture, the next stops it and folds
  // the transcript into the composer. On-device host dictation (Apple Speech)
  // is the primary path; when the host can't capture (non-macOS, helper
  // failure) the browser records the mic itself and the gateway transcribes
  // through the provider's audio endpoint — same button, same states, any
  // platform. The round-trips ("starting", "transcribing") ignore further
  // clicks. Errors (no mic / denied permission) surface as a transcript line.
  const voiceModeRef = useRef<"host" | "browser">("host");
  const toggleVoice = useCallback(async () => {
    if (voice === "idle") {
      setVoice("starting");
      try {
        await startVoice();
        voiceModeRef.current = "host";
        setVoice("recording");
      } catch (hostErr) {
        if (!recorderSupported()) {
          setVoice("idle");
          dispatch({ type: "seam-error", message: `voice: ${errMsg(hostErr)}` });
          return;
        }
        try {
          await startRecording();
          voiceModeRef.current = "browser";
          setVoice("recording");
        } catch (e) {
          setVoice("idle");
          dispatch({ type: "seam-error", message: `voice: ${errMsg(e)}` });
        }
      }
      return;
    }
    if (voice === "recording") {
      setVoice("transcribing");
      try {
        let heard: string;
        if (voiceModeRef.current === "host") {
          heard = (await stopVoice()).trim();
        } else {
          const clip = await stopRecording();
          heard = (await transcribeAudio(clip.data, clip.format)).trim();
        }
        if (heard) {
          setText((prev) => (prev.trim() ? `${prev.trimEnd()} ${heard}` : heard));
        }
      } catch (e) {
        if (voiceModeRef.current === "browser") cancelRecording();
        dispatch({ type: "seam-error", message: `voice: ${errMsg(e)}` });
      } finally {
        setVoice("idle");
        taRef.current?.focus();
      }
    }
  }, [voice]);

  // The slash-command menu (Cursor's popup over the host's prompt templates).
  // Editing or creating a command opens its template file in a prompt-editor
  // panel beside the chat (the same panel mechanics as show).
  const focusComposer = useCallback(() => taRef.current?.focus(), []);
  const openPrompt = useCallback((name: string, path?: string) => {
    handleRef.current.onOpenSurface?.(promptSurface(name, path));
  }, []);
  const slash = useSlashCommands(text, setText, focusComposer, openPrompt);

  // Sends overlap only through the spool await; the guard keeps a double
  // Enter from spooling (and sending) the same attachments twice.
  const sendingRef = useRef(false);
  const usableRef = useRef(usable);
  usableRef.current = usable;
  const submit = useCallback(async () => {
    if (sendingRef.current || !seamRef.current || !usableRef.current) return;
    if (!text.trim() && attachments.length === 0) return;
    sendingRef.current = true;
    try {
      // A voice memo may still be transcribing; its chip spinner is the cue.
      // Waiting here (instead of refusing the send) keeps Enter meaning
      // "send" — the prompt goes out the moment the transcript lands. The
      // resolved transcripts live in state, so re-read through the ref (the
      // closure's `attachments` predates them).
      if (transcribingRef.current.size > 0) {
        await Promise.all([...transcribingRef.current.values()]);
      }
      const atts = attachmentsRef.current.filter((a) => !a.pending);
      let fileLines: string[];
      try {
        fileLines = await spoolAttachments(atts);
      } catch (e) {
        dispatch({ type: "seam-error", message: `attach: ${errMsg(e)}` });
        return;
      }
      const body = composeMessage(text, fileLines);
      const parts = mediaParts(atts);
      if (!body && parts.length === 0) return;
      const echo = body || atts.map((a) => a.name).join(", ") || "(attachment)";
      // Enter while a turn runs queues the message (Cursor's rule); the queue
      // flushes at turn end. Enter while idle starts a turn.
      if (turnActiveRef.current) {
        setQueue((q) => [...q, { qid: qidRef.current++, text: body, parts }]);
        setText("");
        setAttachments([]);
        return;
      }
      if (seamRef.current.prompt(body, parts, hostName(sessionRef.current))) {
        if (chat.items.length === 0) {
          handleRef.current.onTitle?.(text.trim() || atts[0]?.name || echo);
        }
        dispatch({ type: "user", text: body, parts });
        setText("");
        setAttachments([]);
      }
    } finally {
      sendingRef.current = false;
    }
  }, [text, attachments, chat.items.length]);

  // Flush the queue head when the turn ends: sending dispatches a user item
  // and re-arms turnActive, so exactly one queued message runs per turn.
  // A pending edit owns the turn-end it caused — its fork must land before
  // anything else starts a turn, or the server would report busy forever.
  useEffect(() => {
    if (chat.turnActive || !usable || queue.length === 0) return;
    if (pendingEditRef.current) return;
    const head = queue[0];
    if (seamRef.current?.prompt(head.text, head.parts, hostName(sessionRef.current))) {
      setQueue((q) => q.filter((m) => m.qid !== head.qid));
      dispatch({ type: "user", text: head.text, parts: head.parts });
    }
  }, [chat.turnActive, usable, queue]);

  /** Push: send a queued message NOW, steering the in-flight turn onto it. */
  const pushQueued = useCallback(
    (qid: number) => {
      const msg = queue.find((m) => m.qid === qid);
      if (!msg || !seamRef.current) return;
      const name = hostName(sessionRef.current);
      const ok = chat.turnActive
        ? seamRef.current.steer(msg.text, msg.parts, name)
        : seamRef.current.prompt(msg.text, msg.parts, name);
      if (ok) {
        setQueue((q) => q.filter((m) => m.qid !== qid));
        dispatch({ type: "user", text: msg.text, parts: msg.parts });
      }
    },
    [queue, chat.turnActive],
  );

  /**
   * Rewind-and-resubmit (Cursor's edit-a-previous-turn): fork the session
   * just before the edited user message — preserving the original thread —
   * rebind this tab to the branch, and prompt it with the edited text. The
   * fork point comes from a fresh replay; the transcript only truncates once
   * the server confirms the fork (onForked), so a failure changes nothing.
   * Editing mid-turn works like Cursor: the in-flight turn is interrupted,
   * then the fork lands once the engine has wound down (onError retries it
   * while the server still reports busy).
   */
  const resubmitEdit = useCallback(
    async (itemId: number, newText: string) => {
      const seam = seamRef.current;
      const session = sessionRef.current;
      if (!seam || !session) return;
      const userItems = chat.items.filter((it) => it.kind === "user");
      const k = userItems.findIndex((it) => it.id === itemId);
      if (k < 0) return;
      const target = userItems[k];
      const parts = target.parts ?? [];
      try {
        const events = await fetchReplay(session);
        // Locate the edited message in the log: by its replayed seq when it
        // has one, else as the k-th user event. Either way the log entry must
        // match the item's text — an optimistic item whose prompt never
        // landed, or a queued turn driven from another frontend, would skew
        // the mapping and silently fork at the wrong turn otherwise.
        let cut: number;
        if (target.seq != null) {
          cut = events.findIndex((e) => e.type === "user" && e.seq === target.seq);
        } else {
          cut = -1;
          let seen = -1;
          for (let i = 0; i < events.length; i++) {
            if (events[i].type === "user" && ++seen === k) {
              cut = i;
              break;
            }
          }
        }
        const ev = cut >= 0 ? events[cut] : undefined;
        if (!ev || ev.type !== "user" || ev.text !== target.text) {
          throw new Error("transcript is out of sync with the session log — reload the tab");
        }
        // Mid-turn: stop the stream first; the busy-retry in onError covers
        // the gap until the engine has actually wound down.
        if (turnActiveRef.current) seam.interrupt();
        if (!seam.fork(ev.seq - 1)) throw new Error("not connected");
        pendingEditRef.current = {
          throughSeq: ev.seq - 1,
          keep: replayToItems(events.slice(0, cut)),
          text: newText,
          parts,
          retries: 0,
        };
        setEditingId(null);
      } catch (e) {
        dispatch({ type: "seam-error", message: `edit: ${errMsg(e)}` });
      }
    },
    [chat.items],
  );

  // The transcript hooks must be referentially stable across streaming
  // deltas or React.memo(Item) re-renders every row per token anyway. The
  // callbacks read through refs (handleRef / resubmitEditRef), so the object
  // only rebuilds when state the rows genuinely render from changes.
  const resubmitEditRef = useRef(resubmitEdit);
  resubmitEditRef.current = resubmitEdit;
  const onEditStart = useCallback((id: number) => setEditingId(id), []);
  const onEditCancel = useCallback(() => setEditingId(null), []);
  const onEditSubmit = useCallback(
    (id: number, newText: string) => void resubmitEditRef.current(id, newText),
    [],
  );
  // Retry: re-send the most recent user prompt as a fresh turn — the error
  // card's button. Idle + connected only, so it cannot collide with a live
  // turn; the resent message echoes as a new user row, the same as typing it.
  const onRetry = useCallback(() => {
    const seam = seamRef.current;
    if (!seam || !usableRef.current || turnActiveRef.current) return;
    const lastUser = [...itemsRef.current]
      .reverse()
      .find((it) => it.kind === "user");
    if (!lastUser || lastUser.kind !== "user") return;
    if (seam.prompt(lastUser.text, lastUser.parts)) {
      dispatch({ type: "user", text: lastUser.text, parts: lastUser.parts });
    }
  }, []);
  const transcriptHooks = useMemo(
    () => ({
      children: chat.children,
      onOpenChild: (session: string, label: string) =>
        handleRef.current.onOpenRun?.({ session, label }),
      onOpenSurface: (surface: Surface) =>
        handleRef.current.onOpenSurface?.(surface),
      onOpenTerminal: (term: TermRef) =>
        handleRef.current.onOpenTerminal?.(term),
      onOpenPlan: (node: number) => handleRef.current.onOpenPlan?.(node),
      onRetry,
      canRetry: usable && !chat.turnActive,
      selfName,
      edit: {
        editingId,
        canEdit: usable,
        onStart: onEditStart,
        onCancel: onEditCancel,
        onSubmit: onEditSubmit,
      },
    }),
    [
      chat.children,
      chat.turnActive,
      editingId,
      usable,
      onRetry,
      selfName,
      onEditStart,
      onEditCancel,
      onEditSubmit,
    ],
  );

  // Keep selfName synced to the bound session and to live changes from the
  // Share dialog, so the host's own messages (past included) relabel at once —
  // and re-announce the new name so the roster updates for everyone.
  useEffect(() => {
    setSelfName(hostName(boundSession));
    if (!boundSession) return;
    return onHostNameChange(boundSession, (name) => {
      setSelfName(name);
      seamRef.current?.announceName(name);
    });
  }, [boundSession]);

  // Open the People tab the moment this session is shared (the host mints a
  // link) or when a guest arrives via a share link. Also covers a session bound
  // after mount that was shared earlier. openPeople asks the host to open (or
  // focus) the People surface tab — the same panel the top-row People button
  // opens.
  const openPeople = useCallback(() => {
    const sid = sessionRef.current;
    if (!sid) return;
    handleRef.current.onOpenSurface?.(peopleSurface(sid));
  }, []);
  useEffect(() => {
    if (!boundSession) return;
    if (sharedTab || isSharedSession(boundSession)) openPeople();
    return onSharedChange(boundSession, openPeople);
  }, [boundSession, sharedTab, openPeople]);

  /** Edit: pull a queued message back into the composer. */
  const editQueued = useCallback(
    (qid: number) => {
      const msg = queue.find((m) => m.qid === qid);
      if (!msg) return;
      setQueue((q) => q.filter((m) => m.qid !== qid));
      setText(msg.text);
      taRef.current?.focus();
    },
    [queue],
  );

  const answerApproval = useCallback(
    (approved: boolean) => {
      if (!approval) return;
      if (seamRef.current?.approve(approval.requestId, approved)) {
        dispatch({ type: "approval-answered" });
      }
    },
    [approval],
  );

  // Answer (or skip) the ask tool's pending form. The composer's text rides
  // along as "Add more optional details" and clears once delivered; a skip
  // keeps whatever was typed.
  const answerQuestions = useCallback(
    (answers: QuestionAnswer[], skipped: boolean) => {
      if (!questions) return;
      const details = skipped ? "" : text.trim();
      if (
        seamRef.current?.answerQuestions(
          questions.requestId,
          answers,
          details,
          skipped,
        )
      ) {
        dispatch({ type: "questions-answered" });
        if (!skipped) setText("");
      }
    },
    [questions, text],
  );

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    // The command menu owns navigation keys while it is open (Enter on a
    // fully typed command falls through and sends it).
    if (slash.handleKey(e)) return;
    // A pending question form owns the keyboard (Cursor's panel): Enter
    // continues (composer text becomes the optional details), Esc skips, and
    // with an empty composer a bare letter toggles that option.
    if (questions) {
      if (e.key === "Escape") {
        e.preventDefault();
        questionRef.current?.skip();
        return;
      }
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        questionRef.current?.submit();
        return;
      }
      if (
        text === "" &&
        /^[a-zA-Z]$/.test(e.key) &&
        !e.metaKey &&
        !e.ctrlKey &&
        !e.altKey &&
        questionRef.current?.press(e.key)
      ) {
        e.preventDefault();
        return;
      }
    }
    if (approval && text === "") {
      if (e.key === "y" || e.key === "Y") {
        e.preventDefault();
        answerApproval(true);
        return;
      }
      if (e.key === "n" || e.key === "N" || e.key === "Escape") {
        e.preventDefault();
        answerApproval(false);
        return;
      }
    }
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void submit();
    }
    if (e.key === "Escape" && chat.turnActive) {
      e.preventDefault();
      seamRef.current?.interrupt();
    }
  };

  const placeholder = reconnecting
    ? "reconnecting…"
    : questions
      ? "Add more optional details"
      : chat.turnActive
        ? "queue a follow-up (enter) · stop (esc)"
        : "Plan, build, ask anything";

  // A brand-new tab opens Cursor-style: the composer floats centered in the
  // empty page (a touch larger), then drops to the bottom the instant the
  // first transcript item lands (the optimistic user item on Enter). Resumed
  // tabs never start fresh — their history is already on its way.
  const fresh = !resumeId && chat.items.length === 0;

  // A read-only share renders just the transcript — no composer, no
  // interactive cards (queue/approval/questions), nothing to drive with.
  if (readOnly) {
    return (
      <div ref={rootRef} className="flex min-h-0 min-w-0 flex-1 flex-col">
        <ChatView items={chat.items} working={working} hooks={transcriptHooks} />
      </div>
    );
  }

  return (
    <div ref={rootRef} className="flex min-h-0 min-w-0 flex-1">
      <div className="relative flex min-h-0 min-w-0 flex-1 flex-col">
      {!fresh && (
        <ChatView items={chat.items} working={working} hooks={transcriptHooks} />
      )}

      <div
        className={
          fresh
            ? "flex min-h-0 flex-1 items-center justify-center overflow-y-auto py-10"
            : "shrink-0"
        }
      >
        <div className="safe-pb mx-auto w-full max-w-4xl space-y-2 px-3.5 pb-3.5 pt-1">
          {reconnecting && !fresh && (
            <div className="flex items-center gap-2 rounded-md border border-warn/40 bg-warn/10 px-3 py-1.5 text-[12px] text-warn">
              <Loader2 size={12} className="shrink-0 animate-spin" />
              {rebinding
                ? "Reconnecting — waiting for the session to free up…"
                : "Connection lost — reconnecting…"}
            </div>
          )}
          {(chat.jobs.length > 0 || chat.scheduled.length > 0 || runs.length > 0) && (
            <BackgroundBar
              jobs={chat.jobs}
              scheduled={chat.scheduled}
              runs={runs}
              onOpenRun={(r) =>
                handleRef.current.onOpenRun?.({
                  session: r.id,
                  label: r.node ? `Scheduled run · node #${r.node}` : "Scheduled run",
                })
              }
              onOpenJob={(j) =>
                handleRef.current.onOpenTerminal?.({
                  kind: "job",
                  job: j.id,
                  command: j.command,
                })
              }
              onKillJob={(id) => {
                killJob(id)
                  .then(() => dispatch({ type: "job-removed", id }))
                  .catch(() => {});
              }}
              onCancelTask={(id) => {
                cancelPlanNode(id)
                  .then(() => dispatch({ type: "scheduled-removed", id }))
                  .catch(() => {});
              }}
              onStopRun={(r) => {
                stopRun(r.id).catch(() => {});
                // A run spawned by a recurring node keeps re-firing unless the
                // schedule itself is cancelled — offer that once the run stops.
                if (
                  r.node &&
                  window.confirm(
                    `Also cancel the recurring schedule (node #${r.node}) so no new runs spawn?`,
                  )
                ) {
                  cancelPlanNode(r.node)
                    .then(() => dispatch({ type: "scheduled-removed", id: r.node! }))
                    .catch(() => {});
                }
              }}
            />
          )}
          {queue.length > 0 && (
            <QueuedMessages
              queue={queue}
              onPush={pushQueued}
              onEdit={editQueued}
              onDelete={(qid) => setQueue((q) => q.filter((m) => m.qid !== qid))}
            />
          )}
          {approval && (
            <ApprovalCard approval={approval} onAnswer={answerApproval} />
          )}
          {questions && (
            <QuestionCard
              key={questions.requestId}
              request={questions}
              onAnswer={answerQuestions}
              handleRef={questionRef}
            />
          )}

          <div className="relative rounded-[10px] border border-line bg-panel transition-colors focus-within:border-line/0 focus-within:ring-1 focus-within:ring-accent/40">
            {slash.open && (
              <CommandMenu
                commands={slash.matches}
                highlight={slash.highlight}
                below={fresh}
                createName={slash.createName}
                onHover={slash.setHighlight}
                onPick={slash.pick}
                onEdit={slash.pickEdit}
                onCreate={slash.pickCreate}
              />
            )}
            {attachments.length > 0 && (
              <div className="flex flex-wrap gap-1.5 px-2 pt-2">
                {attachments.map((a) => (
                  <span
                    key={a.aid}
                    className="group flex items-center gap-1 rounded-md border border-line bg-card px-2 py-0.5 text-[11px] text-muted"
                  >
                    {a.pending ? (
                      <Loader2 size={11} className="shrink-0 animate-spin text-faint" />
                    ) : a.kind === "image" && a.data ? (
                      <img
                        src={`data:${a.mime};base64,${a.data}`}
                        alt=""
                        className="size-4 shrink-0 rounded-sm object-cover"
                      />
                    ) : (
                      <FileText size={11} className="shrink-0 text-faint" />
                    )}
                    <span className="max-w-[160px] truncate">{a.name}</span>
                    <button
                      type="button"
                      onClick={() =>
                        setAttachments((s) => s.filter((x) => x.aid !== a.aid))
                      }
                      title="Remove"
                      className="flex size-3.5 cursor-pointer items-center justify-center rounded text-faint transition-colors hover:text-red"
                    >
                      <X size={10} />
                    </button>
                  </span>
                ))}
              </div>
            )}
            <textarea
              ref={taRef}
              value={text}
              onChange={(e) => setText(e.target.value)}
              onKeyDown={onKeyDown}
              onPaste={onPaste}
              rows={1}
              placeholder={placeholder}
              // composer-input forces >=16px on touch (index.css) so iOS Safari
              // doesn't zoom on focus; the denser desktop size is kept off touch.
              className={`composer-input block w-full resize-none bg-transparent px-3 pt-2.5 pb-1 leading-relaxed text-bright outline-none placeholder:text-faint ${
                fresh ? "min-h-[76px] text-[15px]" : ""
              }`}
            />
            <input
              ref={fileInputRef}
              type="file"
              multiple
              className="hidden"
              onChange={(e) => {
                onPickFiles(e.target.files);
                e.target.value = "";
              }}
            />
            <div className="flex items-center justify-between px-2 pt-1 pb-2">
              <span className="flex items-center gap-2 select-none">
                <ModelPicker current={model} onSelect={selectModel} />
                <ContextCircle used={usedTokens} total={contextLength} />
              </span>
              <span className="flex items-center gap-1.5">
                <Tooltip side="top" label="Attach files">
                  <button
                    type="button"
                    aria-label="Attach files"
                    onClick={() => fileInputRef.current?.click()}
                    className="tap flex size-6 cursor-pointer items-center justify-center rounded-full text-faint transition-colors hover:bg-hover hover:text-text"
                  >
                    <Paperclip size={14} />
                  </button>
                </Tooltip>
                <Tooltip
                  side="top"
                  label={
                    voice === "recording"
                      ? "Stop dictation"
                      : "Dictate from the host mic"
                  }
                >
                  <button
                    type="button"
                    aria-label={voice === "recording" ? "Stop dictation" : "Dictate"}
                    onClick={toggleVoice}
                    disabled={voice === "starting" || voice === "transcribing"}
                    aria-pressed={voice === "recording"}
                    className={`tap flex size-6 cursor-pointer items-center justify-center rounded-full transition-colors disabled:cursor-default ${
                      voice === "recording"
                        ? "animate-pulse bg-red text-canvas"
                        : "text-faint hover:bg-hover hover:text-text"
                    }`}
                  >
                    {voice === "starting" || voice === "transcribing" ? (
                      <Loader2 size={13} className="animate-spin" />
                    ) : (
                      <Mic size={14} />
                    )}
                  </button>
                </Tooltip>
                {chat.turnActive ? (
                  <Tooltip side="top" label="Stop (esc)">
                    <button
                      type="button"
                      aria-label="Stop"
                      onClick={() => seamRef.current?.interrupt()}
                      className="tap flex size-6 cursor-pointer items-center justify-center rounded-full bg-btn text-canvas transition-opacity hover:opacity-90"
                    >
                      <Square size={9} fill="currentColor" />
                    </button>
                  </Tooltip>
                ) : (
                  <Tooltip side="top" label="Send (enter)">
                    <button
                      type="button"
                      aria-label="Send"
                      onClick={() => void submit()}
                      disabled={!usable || (!text.trim() && attachments.length === 0)}
                      className="tap flex size-6 cursor-pointer items-center justify-center rounded-full bg-btn text-canvas transition-opacity disabled:cursor-default disabled:opacity-30"
                    >
                      <ArrowUp size={14} strokeWidth={2.5} />
                    </button>
                  </Tooltip>
                )}
              </span>
            </div>
          </div>
        </div>
      </div>
      </div>
    </div>
  );
}

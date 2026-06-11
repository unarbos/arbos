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
import { ChatView } from "./ChatView";
import { ChildPanel } from "./ChildPanel";
import { CommandMenu } from "./CommandMenu";
import { ContextCircle } from "./ContextCircle";
import { ModelPicker } from "./ModelPicker";
import { QueuedMessages, type QueuedMessage } from "./QueuedMessages";
import {
  cancelPlanNode,
  fetchChildren,
  fetchModels,
  fetchReplay,
  killJob,
  startVoice,
  stopRun,
  stopVoice,
  type ChildRun,
  type ModelOption,
} from "@/lib/api";
import { useSlashCommands } from "@/lib/useSlashCommands";
import { SeamClient, type ConnectionState } from "@/lib/seam";
import { detailsSurface, type Surface } from "@/lib/surface";
import type { ContentBlock } from "@/lib/types";
import { useAutosize } from "@/lib/useAutosize";
import {
  chatReducer,
  initialChatState,
  replayToItems,
  type TranscriptItem,
} from "@/lib/transcript";

const RECONNECT_MS = 2000;
const REBIND_MS = 1000;
const REBIND_MAX = 5;
const COMPOSER_MAX_PX = 200;
const RUNS_POLL_MS = 5000;
const RUN_REPLAY_POLL_MS = 2000;
// An edit mid-turn: the interrupt needs a beat to wind the engine down before
// the fork lands; retry on "fork: busy" until it does (or give up).
const FORK_RETRY_MS = 250;
const FORK_RETRY_MAX = 8;
/** Cap per-file text folded into a prompt so an attach never blows the turn. */
const ATTACH_MAX_CHARS = 100_000;

/**
 * A file picked into the composer. Text files fold their contents into the
 * prompt as fenced blocks; images ride along as base64 multimodal parts the
 * seam carries to a vision model (ADR-0022).
 */
interface Attachment {
  aid: number;
  name: string;
  kind: "text" | "image";
  text?: string; // kind "text": file contents
  data?: string; // kind "image": base64 payload (no data: prefix)
  mime?: string; // kind "image": MIME type
}

function errMsg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/**
 * Fold text attachments into the prompt the seam carries: each file as a fenced
 * block under its name, then the typed message. Images are not folded — they go
 * as parts. Empty pieces drop out, so an image-only send still works.
 *
 * A slash command must stay at position 0 — the engine's template expansion
 * keys on the leading "/" — so for `/cmd …` the typed text leads and the
 * attachments follow (folding into the command's arguments).
 */
function composeMessage(text: string, attachments: Attachment[]): string {
  const typed = text.trim();
  const files = attachments
    .filter((a) => a.kind === "text")
    .map((a) => `Attached file \`${a.name}\`:\n\n\`\`\`\n${a.text ?? ""}\n\`\`\``);
  const pieces = typed.startsWith("/") ? [typed, ...files] : [...files, typed];
  return pieces.filter(Boolean).join("\n\n");
}

/** The image attachments as multimodal content blocks for the prompt. */
function imageParts(attachments: Attachment[]): ContentBlock[] {
  return attachments
    .filter((a): a is Attachment & { data: string } => a.kind === "image" && !!a.data)
    .map((a) => ({
      type: "image",
      image: { data: a.data, mimeType: a.mime ?? "image/png" },
    }));
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
}: {
  active: boolean;
  focused: boolean;
  /** Bumped by explicit activation (tab click, new tab, split): grab focus. */
  focusTick: number;
  resumeId: string | null;
  handle: ChatTabHandle;
}) {
  const [chat, dispatch] = useReducer(chatReducer, initialChatState);
  const [connState, setConnState] = useState<ConnectionState>("idle");
  const [text, setText] = useState("");
  // Sub-agent windows: which child is open, the chat's scheduled runs (plan
  // node firings, from the gateway), and a polled replay for a child whose
  // events never relayed live (a scheduled wake).
  const [openChild, setOpenChild] = useState<{ session: string; label: string } | null>(null);
  const [runs, setRuns] = useState<ChildRun[]>([]);
  const [childReplay, setChildReplay] = useState<TranscriptItem[]>([]);
  const [boundSession, setBoundSession] = useState<string | null>(resumeId);
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
  // Files picked into the composer (paperclip): their text folds into the next
  // prompt and the chips clear once it's sent.
  const [attachments, setAttachments] = useState<Attachment[]>([]);
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

  useEffect(() => {
    let retry: number | undefined;
    let forkRetry: number | undefined;
    let rebinds = 0;
    let closed = false;
    const seam = new SeamClient({
      onState: (s) => {
        setConnState(s);
        if (s === "open") {
          // First connect opens fresh (or resumes by id); a reconnect
          // re-binds the same session, so a dropped socket never loses the
          // thread.
          seam.open(sessionRef.current ?? undefined);
        }
        if (s === "closed") {
          // Any fork awaiting confirmation died with the socket; the rebind
          // below reattaches the original session, transcript intact.
          pendingEditRef.current = null;
          if (!closed) {
            retry = window.setTimeout(() => seam.connect(), RECONNECT_MS);
          }
        }
      },
      onSession: (id) => {
        rebinds = 0;
        sessionRef.current = id;
        setBoundSession(id);
        handleRef.current.onSession?.(id);
      },
      onForked: () => {
        // The server confirmed a rewind-and-edit fork and rebound this
        // connection to the branch — now it is safe to truncate the visible
        // transcript and restart the thread from the edited message.
        const edit = pendingEditRef.current;
        if (!edit) return;
        pendingEditRef.current = null;
        dispatch({ type: "replay", items: edit.keep });
        if (seam.prompt(edit.text, edit.parts)) {
          dispatch({ type: "user", text: edit.text, parts: edit.parts });
        }
      },
      onEnvelope: (env) => {
        // The agent presenting a file (the show tool) opens its panel the
        // moment the result streams in — live only, top-level only: a
        // resumed transcript renders chips to re-open, and a delegated
        // child's shows stay quiet until clicked.
        if (env.depth === 0 && env.event.kind === "tool_finished") {
          const surface = detailsSurface(env.event.data.result);
          if (surface) handleRef.current.onOpenSurface?.(surface);
        }
        dispatch({ type: "envelope", env });
      },
      onNotice: (text, session) => {
        // A notice addressed to THIS chat always renders here, focused or
        // not — it was claimed for this tab and exists nowhere else. Ambient
        // ones (broadcast-class, or a stale sweep for a chat nobody has
        // open) fan out to every connection; only the focused tab renders
        // those (with a split two tabs are visible, exactly one is focused),
        // so the user sees them exactly once.
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
        // connection's hold on the session. Quietly retry the re-bind; fall
        // back to a fresh session only if it never frees up.
        if (msg.includes("already active") && sessionRef.current) {
          if (rebinds++ < REBIND_MAX) {
            retry = window.setTimeout(
              () => seam.open(sessionRef.current ?? undefined),
              REBIND_MS,
            );
          } else {
            sessionRef.current = null;
            seam.open();
          }
          return;
        }
        dispatch({ type: "seam-error", message: msg });
      },
    });
    seamRef.current = seam;

    if (resumeId) {
      // Paint history before the live connection lands.
      fetchReplay(resumeId)
        .then((events) => dispatch({ type: "replay", items: replayToItems(events) }))
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

  useEffect(() => {
    handleRef.current.onBusy?.(chat.turnActive);
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
  useAutosize(taRef, text, COMPOSER_MAX_PX);

  // Poll this chat's scheduled runs (plan-node agent firings) so they appear
  // as openable tabs. Scoped server-side: only runs whose node THIS chat
  // created come back, so nothing leaks across conversations.
  useEffect(() => {
    if (!active || !boundSession) return;
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
  }, [active, boundSession]);

  // A live (relayed) child renders straight from the reducer's children map;
  // a scheduled wake never relayed, so its transcript is fetched — and
  // re-fetched while the run is active, which is what makes the panel "live".
  const liveChild = openChild ? chat.children[openChild.session] : undefined;
  const openRun = openChild
    ? runs.find((r) => r.id === openChild.session)
    : undefined;
  useEffect(() => {
    if (!openChild || liveChild) return;
    let stop = false;
    const load = () => {
      fetchReplay(openChild.session)
        .then((events) => {
          if (!stop) setChildReplay(replayToItems(events));
        })
        .catch(() => {});
    };
    load();
    if (!openRun?.active) return () => { stop = true; };
    const id = window.setInterval(load, RUN_REPLAY_POLL_MS);
    return () => {
      stop = true;
      window.clearInterval(id);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [openChild?.session, liveChild != null, openRun?.active]);

  const connected = connState === "open";
  const approval = chat.pendingApproval;

  const onPickFiles = useCallback((files: FileList | null) => {
    if (!files) return;
    for (const file of Array.from(files)) {
      if (file.type.startsWith("image/")) {
        // Images go to the model as base64 multimodal parts.
        const reader = new FileReader();
        reader.onload = () => {
          const url = String(reader.result);
          const data = url.slice(url.indexOf(",") + 1);
          setAttachments((a) => [
            ...a,
            {
              aid: aidRef.current++,
              name: file.name,
              kind: "image",
              data,
              mime: file.type,
            },
          ]);
        };
        reader.readAsDataURL(file);
      } else {
        // Everything else is read as text and folded into the prompt.
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
  }, []);

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

  // Switch the model on the live session; only re-label once the seam accepts
  // the frame, so a dropped connection doesn't lie about what's running.
  const selectModel = useCallback((id: string) => {
    if (seamRef.current?.setModel(id)) setModel(id);
  }, []);

  // Toggle dictation: first click starts host capture, the next stops it and
  // folds the transcript into the composer. The host round-trips ("starting",
  // "transcribing") ignore further clicks. Errors (no mic / denied permission)
  // surface as a transcript error line.
  const toggleVoice = useCallback(async () => {
    if (voice === "idle") {
      setVoice("starting");
      try {
        await startVoice();
        setVoice("recording");
      } catch (e) {
        setVoice("idle");
        dispatch({ type: "seam-error", message: `voice: ${errMsg(e)}` });
      }
      return;
    }
    if (voice === "recording") {
      setVoice("transcribing");
      try {
        const heard = (await stopVoice()).trim();
        if (heard) {
          setText((prev) => (prev.trim() ? `${prev.trimEnd()} ${heard}` : heard));
        }
      } catch (e) {
        dispatch({ type: "seam-error", message: `voice: ${errMsg(e)}` });
      } finally {
        setVoice("idle");
        taRef.current?.focus();
      }
    }
  }, [voice]);

  // The slash-command menu (Cursor's popup over the host's prompt templates).
  const focusComposer = useCallback(() => taRef.current?.focus(), []);
  const slash = useSlashCommands(text, setText, focusComposer);

  const submit = useCallback(() => {
    const body = composeMessage(text, attachments);
    const parts = imageParts(attachments);
    if ((!body && parts.length === 0) || !seamRef.current) return;
    const echo = body || attachments.map((a) => a.name).join(", ") || "(attachment)";
    // Enter while a turn runs queues the message (Cursor's rule); the queue
    // flushes at turn end. Enter while idle starts a turn.
    if (chat.turnActive) {
      setQueue((q) => [...q, { qid: qidRef.current++, text: body, parts }]);
      setText("");
      setAttachments([]);
      return;
    }
    if (seamRef.current.prompt(body, parts)) {
      if (chat.items.length === 0) {
        handleRef.current.onTitle?.(text.trim() || attachments[0]?.name || echo);
      }
      dispatch({ type: "user", text: body, parts });
      setText("");
      setAttachments([]);
    }
  }, [text, attachments, chat.turnActive, chat.items.length]);

  // Flush the queue head when the turn ends: sending dispatches a user item
  // and re-arms turnActive, so exactly one queued message runs per turn.
  // A pending edit owns the turn-end it caused — its fork must land before
  // anything else starts a turn, or the server would report busy forever.
  useEffect(() => {
    if (chat.turnActive || connState !== "open" || queue.length === 0) return;
    if (pendingEditRef.current) return;
    const head = queue[0];
    if (seamRef.current?.prompt(head.text, head.parts)) {
      setQueue((q) => q.filter((m) => m.qid !== head.qid));
      dispatch({ type: "user", text: head.text, parts: head.parts });
    }
  }, [chat.turnActive, connState, queue]);

  /** Push: send a queued message NOW, steering the in-flight turn onto it. */
  const pushQueued = useCallback(
    (qid: number) => {
      const msg = queue.find((m) => m.qid === qid);
      if (!msg || !seamRef.current) return;
      const ok = chat.turnActive
        ? seamRef.current.steer(msg.text, msg.parts)
        : seamRef.current.prompt(msg.text, msg.parts);
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

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    // The command menu owns navigation keys while it is open (Enter on a
    // fully typed command falls through and sends it).
    if (slash.handleKey(e)) return;
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
      submit();
    }
    if (e.key === "Escape" && chat.turnActive) {
      e.preventDefault();
      seamRef.current?.interrupt();
    }
  };

  const placeholder = !connected
    ? "connecting…"
    : chat.turnActive
      ? "queue a follow-up (enter) · stop (esc)"
      : "Plan, build, ask anything";

  // A brand-new tab opens Cursor-style: the composer sits at the top of the
  // empty page, then drops to the bottom the instant the first transcript
  // item lands (the optimistic user item on Enter). Resumed tabs never start
  // fresh — their history is already on its way.
  const fresh = !resumeId && chat.items.length === 0;

  // A sub-agent opened from the transcript takes over the tab — the child's
  // chat renders as a normal full-width panel (back header, same transcript
  // column), not a side sheet. Esc / back returns to the parent.
  if (openChild) {
    return (
      <div className={active ? "flex min-h-0 flex-1 flex-col" : "hidden"}>
        <ChildPanel
          title={openChild.label}
          items={liveChild ? liveChild.items : childReplay}
          running={liveChild ? liveChild.turnActive : (openRun?.active ?? false)}
          onClose={() => setOpenChild(null)}
        />
      </div>
    );
  }

  return (
    <div ref={rootRef} className="flex min-h-0 flex-1 flex-col">
      <ChatView
        items={chat.items}
        hooks={{
          children: chat.children,
          onOpenChild: (session, label) => {
            setChildReplay([]);
            setOpenChild({ session, label });
          },
          onOpenSurface: (surface) => handleRef.current.onOpenSurface?.(surface),
          edit: {
            editingId,
            canEdit: connected,
            onStart: setEditingId,
            onCancel: () => setEditingId(null),
            onSubmit: (id, text) => void resubmitEdit(id, text),
          },
        }}
      />

      <div className={fresh ? "order-first shrink-0 pt-2" : "shrink-0"}>
        <div className="mx-auto w-full max-w-4xl space-y-2 px-3.5 pb-3.5 pt-1">
          {(chat.jobs.length > 0 || chat.scheduled.length > 0 || runs.length > 0) && (
            <BackgroundBar
              jobs={chat.jobs}
              scheduled={chat.scheduled}
              runs={runs}
              onOpenRun={(r) => {
                setChildReplay([]);
                setOpenChild({
                  session: r.id,
                  label: r.node ? `Scheduled run · node #${r.node}` : "Scheduled run",
                });
              }}
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

          <div className="relative rounded-[10px] border border-line bg-panel transition-colors focus-within:border-line/0 focus-within:ring-1 focus-within:ring-accent/40">
            {slash.open && (
              <CommandMenu
                commands={slash.matches}
                highlight={slash.highlight}
                below={fresh}
                onHover={slash.setHighlight}
                onPick={slash.pick}
              />
            )}
            {attachments.length > 0 && (
              <div className="flex flex-wrap gap-1.5 px-2 pt-2">
                {attachments.map((a) => (
                  <span
                    key={a.aid}
                    className="group flex items-center gap-1 rounded-md border border-line bg-card px-2 py-0.5 text-[11px] text-muted"
                  >
                    {a.kind === "image" && a.data ? (
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
              rows={1}
              placeholder={placeholder}
              className="block w-full resize-none bg-transparent px-3 pt-2.5 pb-1 leading-relaxed text-bright outline-none placeholder:text-faint"
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
            <div className="flex items-center justify-between px-2 pb-2">
              <span className="flex items-center gap-2 select-none">
                <ModelPicker current={model} onSelect={selectModel} />
                <ContextCircle used={usedTokens} total={contextLength} />
              </span>
              <span className="flex items-center gap-1.5">
                <button
                  type="button"
                  onClick={() => fileInputRef.current?.click()}
                  title="Attach files"
                  className="flex size-6 cursor-pointer items-center justify-center rounded-full text-faint transition-colors hover:bg-hover hover:text-text"
                >
                  <Paperclip size={14} />
                </button>
                <button
                  type="button"
                  onClick={toggleVoice}
                  disabled={voice === "starting" || voice === "transcribing"}
                  aria-pressed={voice === "recording"}
                  title={
                    voice === "recording"
                      ? "Stop dictation"
                      : "Dictate from the host mic"
                  }
                  className={`flex size-6 cursor-pointer items-center justify-center rounded-full transition-colors disabled:cursor-default ${
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
                {chat.turnActive ? (
                  <button
                    type="button"
                    onClick={() => seamRef.current?.interrupt()}
                    title="Stop (esc)"
                    className="flex size-6 cursor-pointer items-center justify-center rounded-full bg-btn text-canvas transition-opacity hover:opacity-90"
                  >
                    <Square size={9} fill="currentColor" />
                  </button>
                ) : (
                  <button
                    type="button"
                    onClick={submit}
                    disabled={!connected || (!text.trim() && attachments.length === 0)}
                    title="Send (enter)"
                    className="flex size-6 cursor-pointer items-center justify-center rounded-full bg-btn text-canvas transition-opacity disabled:cursor-default disabled:opacity-30"
                  >
                    <ArrowUp size={14} strokeWidth={2.5} />
                  </button>
                )}
              </span>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

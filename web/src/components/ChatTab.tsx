import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from "react";
import {
  ArrowUp,
  ChevronRight,
  Clock,
  FileText,
  Loader2,
  Mic,
  Orbit,
  Paperclip,
  Pencil,
  Square,
  SquareTerminal,
  Trash2,
  X,
} from "lucide-react";

import { ChatView } from "./ChatView";
import { ChildPanel } from "./ChildPanel";
import { ContextCircle } from "./ContextCircle";
import { ModelPicker } from "./ModelPicker";
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
import { argsPreview } from "@/lib/format";
import { SeamClient, type ConnectionState } from "@/lib/seam";
import type { ContentBlock } from "@/lib/types";
import {
  chatReducer,
  initialChatState,
  replayToItems,
  type BackgroundJob,
  type PendingApproval,
  type ScheduledTask,
  type TranscriptItem,
} from "@/lib/transcript";

const RECONNECT_MS = 2000;
const REBIND_MS = 1000;
const REBIND_MAX = 5;
const COMPOSER_MAX_PX = 200;
const RUNS_POLL_MS = 5000;
const RUN_REPLAY_POLL_MS = 2000;
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
 */
function composeMessage(text: string, attachments: Attachment[]): string {
  const files = attachments
    .filter((a) => a.kind === "text")
    .map((a) => `Attached file \`${a.name}\`:\n\n\`\`\`\n${a.text ?? ""}\n\`\`\``);
  return [...files, text.trim()].filter(Boolean).join("\n\n");
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
}

/**
 * One agent session: its own seam connection, transcript, and composer.
 * Enter sends; while a turn runs Enter steers (replaces the in-flight turn)
 * and Esc interrupts. Approvals answer with a click or y / n.
 *
 * A tab given `resumeId` seeds its transcript from the session's persisted
 * history, then binds the live seam to the same id.
 */
export function ChatTab({
  active,
  resumeId,
  handle,
}: {
  active: boolean;
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
  const [queue, setQueue] = useState<
    { qid: number; text: string; parts: ContentBlock[] }[]
  >([]);
  const qidRef = useRef(1);
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
  const taRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const handleRef = useRef(handle);
  handleRef.current = handle;
  const activeRef = useRef(active);
  activeRef.current = active;

  useEffect(() => {
    let retry: number | undefined;
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
        if (s === "closed" && !closed) {
          retry = window.setTimeout(() => seam.connect(), RECONNECT_MS);
        }
      },
      onSession: (id) => {
        rebinds = 0;
        sessionRef.current = id;
        setBoundSession(id);
        handleRef.current.onSession?.(id);
      },
      onEnvelope: (env) => dispatch({ type: "envelope", env }),
      onNotice: (text, session) => {
        // A notice addressed to THIS chat always renders here, focused or
        // not — it was claimed for this tab and exists nowhere else. Ambient
        // ones (broadcast-class, or a stale sweep for a chat nobody has
        // open) fan out to every connection; only the visible tab renders
        // those, so the user sees them exactly once.
        if (session === sessionRef.current || activeRef.current) {
          dispatch({ type: "notice", text });
        }
      },
      onError: (msg) => {
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
      seam.close();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    handleRef.current.onBusy?.(chat.turnActive);
  }, [chat.turnActive]);

  // The page is the prompt: clicking anywhere in the active tab focuses it.
  // But a click landing on its own focusable control (a popup's search box,
  // another input) must keep that focus — otherwise the composer steals every
  // keystroke. `[data-keep-focus]` lets a popup opt its whole subtree out.
  useEffect(() => {
    if (!active) return;
    const focus = (e: MouseEvent) => {
      if (window.getSelection()?.toString()) return; // don't steal a selection
      const target = e.target as HTMLElement | null;
      if (
        target?.closest(
          'input, textarea, select, [contenteditable="true"], [data-keep-focus]',
        )
      ) {
        return;
      }
      taRef.current?.focus();
    };
    document.addEventListener("click", focus);
    taRef.current?.focus();
    return () => document.removeEventListener("click", focus);
  }, [active]);

  // Autosize the composer to its content, capped so it never eats the page.
  useEffect(() => {
    const ta = taRef.current;
    if (!ta) return;
    ta.style.height = "auto";
    ta.style.height = `${Math.min(ta.scrollHeight, COMPOSER_MAX_PX)}px`;
  }, [text]);

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
  useEffect(() => {
    if (chat.turnActive || connState !== "open" || queue.length === 0) return;
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

  return (
    <div className={active ? "flex min-h-0 flex-1 flex-col" : "hidden"}>
      <ChatView
        items={chat.items}
        hooks={{
          children: chat.children,
          onOpenChild: (session, label) => {
            setChildReplay([]);
            setOpenChild({ session, label });
          },
        }}
      />

      {openChild && (
        <ChildPanel
          title={openChild.label}
          items={liveChild ? liveChild.items : childReplay}
          running={liveChild ? liveChild.turnActive : (openRun?.active ?? false)}
          onClose={() => setOpenChild(null)}
        />
      )}

      <div className="shrink-0">
        <div className="mx-auto w-full max-w-3xl space-y-2 px-3.5 pb-3.5 pt-1">
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

          <div className="rounded-[10px] border border-line bg-panel transition-colors focus-within:border-line/0 focus-within:ring-1 focus-within:ring-accent/40">
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
                <button
                  type="button"
                  onClick={() => fileInputRef.current?.click()}
                  title="Attach files"
                  className="flex size-6 cursor-pointer items-center justify-center rounded-full text-faint transition-colors hover:bg-white/[0.06] hover:text-text"
                >
                  <Paperclip size={14} />
                </button>
                <ModelPicker current={model} onSelect={selectModel} />
                <ContextCircle used={usedTokens} total={contextLength} />
              </span>
              <span className="flex items-center gap-1.5">
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
                      : "text-faint hover:bg-white/[0.06] hover:text-text"
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

/**
 * Messages waiting their turn, Cursor-style: quiet cards under the transcript
 * with hover affordances — edit (back into the composer), push (send now,
 * steering the in-flight turn), delete.
 */
function QueuedMessages({
  queue,
  onPush,
  onEdit,
  onDelete,
}: {
  queue: { qid: number; text: string; parts: ContentBlock[] }[];
  onPush: (qid: number) => void;
  onEdit: (qid: number) => void;
  onDelete: (qid: number) => void;
}) {
  return (
    <div className="space-y-1">
      {queue.map((m) => (
        <div
          key={m.qid}
          className="group flex items-center gap-2 rounded-md border border-line/60 bg-card/60 px-3 py-1.5"
        >
          <span className="min-w-0 flex-1 truncate text-muted">
            {m.text ||
              `${m.parts.length} image${m.parts.length === 1 ? "" : "s"}`}
          </span>
          <span className="flex shrink-0 items-center gap-1 opacity-0 transition-opacity group-hover:opacity-100">
            <button
              type="button"
              onClick={() => onEdit(m.qid)}
              title="Edit"
              className="flex size-5 cursor-pointer items-center justify-center rounded text-faint transition-colors hover:bg-white/[0.06] hover:text-text"
            >
              <Pencil size={11} />
            </button>
            <button
              type="button"
              onClick={() => onPush(m.qid)}
              title="Send now"
              className="flex size-5 cursor-pointer items-center justify-center rounded text-faint transition-colors hover:bg-white/[0.06] hover:text-text"
            >
              <ArrowUp size={12} />
            </button>
            <button
              type="button"
              onClick={() => onDelete(m.qid)}
              title="Delete"
              className="flex size-5 cursor-pointer items-center justify-center rounded text-faint transition-colors hover:bg-white/[0.06] hover:text-red"
            >
              <Trash2 size={11} />
            </button>
          </span>
        </div>
      ))}
    </div>
  );
}

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
function BackgroundBar({
  jobs,
  scheduled,
  runs,
  onOpenRun,
  onKillJob,
  onCancelTask,
  onStopRun,
}: {
  jobs: BackgroundJob[];
  scheduled: ScheduledTask[];
  runs: ChildRun[];
  onOpenRun: (r: ChildRun) => void;
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
              <SquareTerminal size={12} className="shrink-0 text-faint" />
              <span className="min-w-0 flex-1 truncate font-mono text-[11.5px]">
                {j.command || j.id}
              </span>
              <button
                type="button"
                onClick={() => onKillJob(j.id)}
                title={`Kill ${j.id}`}
                className="flex size-5 shrink-0 cursor-pointer items-center justify-center rounded text-faint opacity-0 transition-all group-hover:opacity-100 hover:bg-white/[0.06] hover:text-red"
              >
                <X size={11} />
              </button>
            </div>
          ))}
          {scheduled.map((t) => (
            <div
              key={t.id}
              className="group flex min-w-0 items-center gap-2 text-[12px] text-muted"
            >
              <Clock size={12} className="shrink-0 text-faint" />
              <span className="min-w-0 flex-1 truncate">
                {t.goal} <span className="text-faint">· {t.when}</span>
              </span>
              <button
                type="button"
                onClick={() => onCancelTask(t.id)}
                title={`Cancel #${t.id}`}
                className="flex size-5 shrink-0 cursor-pointer items-center justify-center rounded text-faint opacity-0 transition-all group-hover:opacity-100 hover:bg-white/[0.06] hover:text-red"
              >
                <X size={11} />
              </button>
            </div>
          ))}
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
                <Orbit size={12} className="shrink-0 text-accent" />
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
                className="flex size-5 shrink-0 cursor-pointer items-center justify-center rounded text-faint opacity-0 transition-all group-hover:opacity-100 hover:bg-white/[0.06] hover:text-red"
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

function ApprovalCard({
  approval,
  onAnswer,
}: {
  approval: PendingApproval;
  onAnswer: (approved: boolean) => void;
}) {
  return (
    <div className="rounded-[10px] border border-line bg-card px-3 py-2.5">
      <div className="break-words font-mono text-[11.5px]">
        <span className="text-bright">{approval.call.Name}</span>{" "}
        <span className="text-muted">{argsPreview(approval.call, 200)}</span>
      </div>
      {approval.reason && (
        <div className="mt-0.5 text-[12px] text-muted">{approval.reason}</div>
      )}
      <div className="mt-2 flex items-center gap-2">
        <button
          type="button"
          onClick={() => onAnswer(true)}
          className="cursor-pointer rounded-md bg-btn px-3 py-0.5 text-[12px] font-medium text-canvas transition-opacity hover:opacity-90"
        >
          Run
        </button>
        <button
          type="button"
          onClick={() => onAnswer(false)}
          className="cursor-pointer rounded-md border border-line px-3 py-0.5 text-[12px] text-muted transition-colors hover:text-text"
        >
          Skip
        </button>
        <span className="ml-1 text-[11px] text-faint select-none">y / n</span>
      </div>
    </div>
  );
}

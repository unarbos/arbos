import { useCallback, useEffect, useReducer, useRef, useState } from "react";

import { ChatView } from "./components/ChatView";
import { argsPreview } from "./lib/format";
import { SeamClient, type ConnectionState } from "./lib/seam";
import { chatReducer, initialChatState } from "./lib/transcript";

const RECONNECT_MS = 2000;

/**
 * The web TUI: one session, one column, one prompt. Enter sends; while a turn
 * runs Enter steers (replaces the in-flight turn, the TUI rule) and Esc
 * interrupts. Approvals answer with y / n. Nothing else.
 */
export default function App() {
  const [chat, dispatch] = useReducer(chatReducer, initialChatState);
  const [connState, setConnState] = useState<ConnectionState>("idle");
  const [text, setText] = useState("");

  const seamRef = useRef<SeamClient | null>(null);
  const sessionRef = useRef<string | null>(null);
  const taRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    let retry: number | undefined;
    const seam = new SeamClient({
      onState: (s) => {
        setConnState(s);
        if (s === "open") {
          // First connect opens fresh; a reconnect re-binds the same session
          // (open resumes by id), so a dropped socket never loses the thread.
          seam.open(sessionRef.current ?? undefined);
        }
        if (s === "closed") {
          retry = window.setTimeout(() => seam.connect(), RECONNECT_MS);
        }
      },
      onSession: (id) => {
        sessionRef.current = id;
      },
      onEnvelope: (env) => dispatch({ type: "envelope", env }),
      onError: (msg) => dispatch({ type: "seam-error", message: msg }),
    });
    seamRef.current = seam;
    seam.connect();
    return () => {
      window.clearTimeout(retry);
      seam.close();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // The page is the terminal: clicking anywhere focuses the prompt.
  useEffect(() => {
    const focus = () => {
      if (window.getSelection()?.toString()) return; // don't steal a selection
      taRef.current?.focus();
    };
    document.addEventListener("click", focus);
    return () => document.removeEventListener("click", focus);
  }, []);

  const connected = connState === "open";
  const approval = chat.pendingApproval;

  const submit = useCallback(() => {
    const t = text.trim();
    if (!t || !seamRef.current) return;
    // The TUI rule: input while a turn runs steers it (silently replaces the
    // in-flight work); input while idle starts a turn.
    const ok = chat.turnActive
      ? seamRef.current.steer(t)
      : seamRef.current.prompt(t);
    if (ok) {
      dispatch({ type: "user", text: t });
      setText("");
    }
  }, [text, chat.turnActive]);

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

  return (
    <div className="flex flex-col h-full min-h-0">
      <ChatView items={chat.items} ephemeral={chat.ephemeral} />

      <div className="border-t border-deep/50">
        <div className="max-w-2xl mx-auto px-4 py-3 space-y-2">
          {approval && (
            <div className="text-[0.85em] text-warn break-words">
              approve <span className="text-text">{approval.call.Name}</span>{" "}
              <span className="text-muted">{argsPreview(approval.call)}</span> —{" "}
              <button
                type="button"
                onClick={() => answerApproval(true)}
                className="text-primary underline underline-offset-2 cursor-pointer"
              >
                y
              </button>
              {" / "}
              <button
                type="button"
                onClick={() => answerApproval(false)}
                className="text-danger underline underline-offset-2 cursor-pointer"
              >
                n
              </button>
            </div>
          )}

          <div className="flex items-start gap-2">
            <span
              className={`select-none pt-px ${
                connected ? "text-primary" : "text-danger animate-pulse"
              }`}
            >
              ›
            </span>
            <textarea
              ref={taRef}
              value={text}
              onChange={(e) => setText(e.target.value)}
              onKeyDown={onKeyDown}
              rows={Math.min(6, Math.max(1, text.split("\n").length))}
              placeholder={
                !connected
                  ? "connecting…"
                  : chat.turnActive
                    ? "steer (enter) · interrupt (esc)"
                    : ""
              }
              autoFocus
              className="flex-1 resize-none bg-transparent text-text placeholder:text-muted/50 outline-none border-none p-0 m-0 leading-relaxed"
            />
            {chat.turnActive && (
              <span className="text-accent text-[0.8em] animate-pulse select-none pt-px">
                working
              </span>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

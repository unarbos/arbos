import { useCallback, useEffect, useRef, useState } from "react";
import { ArrowUp } from "lucide-react";

import { ChatView } from "./ChatView";
import { fetchReplay, type MatrixSession } from "@/lib/api";
import { MatrixClient, type MatrixState, type RoomEvent } from "@/lib/matrix";
import { replayToItems, type TranscriptItem } from "@/lib/transcript";
import { useAutosize } from "@/lib/useAutosize";

/**
 * MatrixRoomView — a share guest's chat rendered as a Matrix client (ADR-0041
 * P2.2/P2.3): the browser reads and drives the session's room directly as the
 * guest's own Matrix identity (the token handed to it in /api/me), instead of
 * the bespoke control seam. Membership is the authority — the token works only
 * while the guest is a joined member, so a kick (revoke) ends the stream.
 *
 * History comes from the session's replay endpoint (seeded once); live messages
 * arrive on the Matrix /sync loop. The guest's own messages are shown
 * optimistically on send and suppressed when they echo back from sync (matched
 * by sender) — the same "skip your own echo" rule the seam uses by origin. A
 * sent message is a plain room message, so the agent's room watcher turns it
 * into a turn and the reply mirrors back here on the next sync.
 */
export function MatrixRoomView({
  session,
  sessionId,
  canWrite,
}: {
  /** The guest's own Matrix credentials + room coordinates. */
  session: MatrixSession;
  /** The arbos session id the share is scoped to — keys the replay history. */
  sessionId: string;
  canWrite: boolean;
}) {
  const [items, setItems] = useState<TranscriptItem[]>([]);
  const [conn, setConn] = useState<MatrixState>("idle");
  const [text, setText] = useState("");
  const idRef = useRef(1);
  const taRef = useRef<HTMLTextAreaElement>(null);
  const clientRef = useRef<MatrixClient | null>(null);

  useAutosize(taRef, text, 200);

  const append = useCallback((make: (id: number) => TranscriptItem) => {
    setItems((prev) => [...prev, make(idRef.current++)]);
  }, []);

  useEffect(() => {
    let closed = false;

    // Seed history from the session's replay (the guest is authorized for this
    // one session's replay), then keep the id counter ahead of the live stream.
    void (async () => {
      try {
        const events = await fetchReplay(sessionId);
        if (!closed && events.length > 0) {
          const seeded = replayToItems(events);
          idRef.current = seeded.length + 1;
          setItems(seeded);
        }
      } catch {
        /* no replay access (or none yet): start from the live stream */
      }
    })();

    const client = new MatrixClient(session, {
      onState: (s) => {
        if (!closed) setConn(s);
      },
      onEvents: (events: RoomEvent[]) => {
        if (closed) return;
        for (const ev of events) {
          // Skip our own echo (shown optimistically on send).
          if (ev.sender === session.user_id) continue;
          // Human-to-human side notes fold inline as a side-note row (ADR-0038).
          if (ev.kind === "chat_note" || ev.type === "life.arbos.chat_note") {
            append((id) => ({
              kind: "chatnote",
              id,
              text: ev.body,
              author: ev.author ?? localpart(ev.sender),
            }));
            continue;
          }
          const isAssistant = ev.role === "assistant" || ev.kind === "assistant_message";
          if (isAssistant) {
            append((id) => ({ kind: "assistant", id, text: ev.body, streaming: false }));
          } else {
            append((id) => ({
              kind: "user",
              id,
              text: ev.body,
              author: ev.author ?? localpart(ev.sender),
            }));
          }
        }
      },
      onError: () => {
        /* transient sync failures self-heal; nothing to surface for a guest */
      },
    });
    clientRef.current = client;
    client.start();
    return () => {
      closed = true;
      client.stop();
      clientRef.current = null;
    };
  }, [session, sessionId, append]);

  const send = useCallback(() => {
    const body = text.trim();
    const client = clientRef.current;
    if (!body || !client || !canWrite) return;
    // Optimistic: show it immediately; the sync echo is suppressed by sender.
    append((id) => ({ kind: "user", id, text: body, author: localpart(session.user_id) }));
    setText("");
    void client.send(body).catch(() => {});
    taRef.current?.focus();
  }, [text, canWrite, append, session.user_id]);

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      send();
    }
  };

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <ChatView items={items} working={false} hooks={{ selfName: localpart(session.user_id) }} />
      {canWrite && (
        <div
          className="shrink-0"
          style={{ paddingBottom: "calc(env(safe-area-inset-bottom, 0px) + 1.25rem)" }}
        >
          <div className="mx-auto w-full max-w-4xl px-3.5 pt-1">
            <div className="relative -mx-3 rounded-[10px] border border-line bg-panel transition-colors focus-within:ring-1 focus-within:ring-accent/40">
              <textarea
                ref={taRef}
                value={text}
                onChange={(e) => setText(e.target.value)}
                onKeyDown={onKeyDown}
                rows={1}
                placeholder={conn === "syncing" ? "Message the room" : "Connecting to the room…"}
                className="composer-input block w-full resize-none bg-transparent px-3 pt-2.5 pb-1 leading-relaxed text-bright outline-none placeholder:text-faint"
              />
              <div className="flex items-center justify-end px-2 pt-1 pb-2">
                <button
                  type="button"
                  aria-label="Send"
                  onClick={send}
                  disabled={!text.trim()}
                  className="tap flex size-6 cursor-pointer items-center justify-center rounded-full bg-btn text-canvas transition-opacity disabled:cursor-default disabled:opacity-30"
                >
                  <ArrowUp size={14} strokeWidth={2.5} />
                </button>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

/** The localpart of a Matrix user id ("@guest-alice:host" -> "guest-alice"). */
function localpart(userID: string): string {
  if (!userID.startsWith("@")) return userID;
  const rest = userID.slice(1);
  const i = rest.indexOf(":");
  return i >= 0 ? rest.slice(0, i) : rest;
}

import { useEffect, useRef, useState } from "react";

import { PeopleChat } from "./PeopleChat";
import { hostName } from "@/lib/identity";
import { SeamClient } from "@/lib/seam";

/**
 * The People surface: the human-to-human side chat for the collaborators on a
 * board, rendered as its own tab beside the conversation. It owns a scoped
 * seam connection to its session (surface.path is the session id) so it lives
 * independently of the chat tab — open it alongside a canvas, the files, or
 * anything else, and keep more than one companion panel up at once.
 *
 * The seam here only carries presence + side-chat lines: it binds the session
 * (which registers this door in the roster and replays the chat_note log), and
 * never starts a turn. The agent transcript stays in the ChatTab's own seam.
 */
export function PeopleSurface({
  surface,
  readOnly,
}: {
  surface: { path: string };
  /** Present for parity with other surfaces; the seam binds regardless. */
  active?: boolean;
  readOnly?: boolean;
}) {
  const sessionId = surface.path;
  const [roster, setRoster] = useState<string[]>([]);
  const [typing, setTyping] = useState<string[]>([]);
  const [connected, setConnected] = useState(false);

  const seamRef = useRef<SeamClient | null>(null);
  const selfName = hostName(sessionId);
  const selfNameRef = useRef(selfName);
  selfNameRef.current = selfName;
  const typingTimers = useRef<Map<string, number>>(new Map());
  const lastTypingPing = useRef(0);

  // One scoped seam per session: bind it (registering us in the roster), then
  // route presence frames here. Side notes themselves render inline in the
  // conversation timeline (ADR-0038), so this panel only carries presence +
  // posting — it no longer mirrors the message list. Reconnects re-bind and
  // re-announce automatically.
  useEffect(() => {
    let closed = false;
    let connectAttempts = 0;
    let retry: number | undefined;
    const timers = typingTimers.current;

    const noteTyping = (name: string) => {
      if (name === selfNameRef.current) return;
      setTyping((t) => (t.includes(name) ? t : [...t, name]));
      const prev = timers.get(name);
      if (prev) window.clearTimeout(prev);
      timers.set(
        name,
        window.setTimeout(() => {
          timers.delete(name);
          setTyping((t) => t.filter((n) => n !== name));
        }, 3000),
      );
    };

    const seam = new SeamClient({
      onState: (s) => {
        setConnected(s === "open");
        if (s === "open") {
          connectAttempts = 0;
          seam.open(sessionId);
        }
        if (s === "closed" && !closed) {
          retry = window.setTimeout(
            () => seam.connect(),
            Math.min(1000 * 2 ** connectAttempts++, 15000),
          );
        }
      },
      onSession: (id) => {
        if (id !== sessionId) return;
        seam.announceName(hostName(id));
      },
      onRoster: (sess, users) => {
        // An empty room arrives as JSON null (a Go nil slice), so default it —
        // setRoster(null) would crash the roster .filter on the next render.
        if (sess === sessionId) setRoster(users ?? []);
      },
      onTyping: (sess, user) => {
        if (sess === sessionId) noteTyping(user);
      },
    });
    seamRef.current = seam;
    seam.connect();

    return () => {
      closed = true;
      if (retry) window.clearTimeout(retry);
      for (const id of timers.values()) window.clearTimeout(id);
      timers.clear();
      seamRef.current = null;
      seam.close();
    };
  }, [sessionId]);

  const send = (text: string) => {
    // The note broadcasts to the session; it renders inline in the conversation
    // (and for everyone else) rather than in a list here.
    seamRef.current?.chatNote(text, hostName(sessionId));
  };

  // Throttled typing ping: at most one per ~1.5s while actively typing.
  const emitTyping = () => {
    const now = Date.now();
    if (now - lastTypingPing.current < 1500) return;
    lastTypingPing.current = now;
    seamRef.current?.typingPing();
  };

  return (
    <PeopleChat
      selfName={selfName}
      canPost={!readOnly && connected}
      roster={roster}
      typing={typing}
      onSend={send}
      onTyping={emitTyping}
    />
  );
}

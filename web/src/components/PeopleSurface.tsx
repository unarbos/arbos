import { useEffect, useRef, useState } from "react";

import { PeopleChat } from "./PeopleChat";
import { fetchReplay } from "@/lib/api";
import { hostName } from "@/lib/identity";
import { SeamClient } from "@/lib/seam";
import { peopleFromReplay, type PeopleMessage } from "@/lib/transcript";

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
  const [messages, setMessages] = useState<PeopleMessage[]>([]);
  const [roster, setRoster] = useState<string[]>([]);
  const [typing, setTyping] = useState<string[]>([]);
  const [connected, setConnected] = useState(false);

  const idRef = useRef(1);
  const seamRef = useRef<SeamClient | null>(null);
  const selfName = hostName(sessionId);
  const selfNameRef = useRef(selfName);
  selfNameRef.current = selfName;
  const typingTimers = useRef<Map<string, number>>(new Map());
  const lastTypingPing = useRef(0);

  // One scoped seam per session: bind it (replaying chat_notes from the log and
  // registering us in the roster), then route presence + side-chat frames here.
  // Own lines are echo-filtered server-side, so an inbound chat_note is always
  // from someone else. Reconnects re-bind and re-announce automatically.
  useEffect(() => {
    let closed = false;
    let connectAttempts = 0;
    let retry: number | undefined;
    const timers = typingTimers.current;

    const replay = () => {
      fetchReplay(sessionId)
        .then((events) => {
          if (closed) return;
          const ppl = peopleFromReplay(events);
          idRef.current = ppl.length + 1;
          setMessages(ppl);
        })
        .catch(() => {});
    };

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
        replay();
      },
      onEnvelope: (env) => {
        if (env.depth === 0 && env.event.kind === "chat_note") {
          const d = env.event.data;
          setMessages((p) => [
            ...p,
            { id: idRef.current++, text: d.text, author: d.author ?? "" },
          ]);
        }
      },
      onRoster: (sess, users) => {
        if (sess === sessionId) setRoster(users);
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
    const name = hostName(sessionId);
    if (seamRef.current?.chatNote(text, name)) {
      setMessages((p) => [...p, { id: idRef.current++, text, author: name }]);
    }
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
      messages={messages}
      selfName={selfName}
      canPost={!readOnly && connected}
      roster={roster}
      typing={typing}
      onSend={send}
      onTyping={emitTyping}
    />
  );
}

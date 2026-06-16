import { useEffect, useRef, useState } from "react";
import { X } from "lucide-react";

import type { PeopleMessage } from "@/lib/transcript";

/**
 * The People panel: a human-to-human side chat for collaborators on this board.
 * It is NOT the agent conversation — messages here go to the other people on
 * the session, never to the model. Presentational: the parent (ChatTab) owns
 * the message state and the seam send. Following the chat convention, you see
 * other participants' names but never your own.
 */
export function PeopleChat({
  messages,
  selfName,
  canPost,
  onSend,
  onClose,
}: {
  messages: PeopleMessage[];
  selfName: string;
  canPost: boolean;
  onSend: (text: string) => void;
  onClose: () => void;
}) {
  const [draft, setDraft] = useState("");
  const endRef = useRef<HTMLDivElement>(null);

  // Follow the tail as new lines arrive.
  useEffect(() => {
    endRef.current?.scrollIntoView({ block: "end" });
  }, [messages]);

  const send = () => {
    const t = draft.trim();
    if (!t) return;
    onSend(t);
    setDraft("");
  };

  return (
    <div className="flex h-full w-full flex-col border-l border-line bg-panel">
      <div className="flex shrink-0 items-center justify-between border-b border-line px-3 py-2">
        <span className="text-[12px] font-semibold text-bright">People</span>
        <button
          type="button"
          onClick={onClose}
          title="Hide people chat"
          className="flex size-6 items-center justify-center rounded-md text-muted hover:bg-hover hover:text-text"
        >
          <X size={13} />
        </button>
      </div>

      <div className="min-h-0 flex-1 space-y-2 overflow-y-auto px-3 py-2">
        {messages.length === 0 && (
          <div className="text-[12px] leading-relaxed text-faint">
            Side chat for people on this board. Messages here go to the others —
            the agent doesn’t see them.
          </div>
        )}
        {messages.map((m) => {
          // See others' names, never your own (matches the agent chat). Your own
          // lines either have no author or carry your name.
          const mine = !m.author || m.author === selfName;
          return (
            <div key={m.id} className={mine ? "text-right" : ""}>
              {!mine && (
                <div className="text-[10.5px] uppercase tracking-wider text-faint select-none">
                  {m.author}
                </div>
              )}
              <div className="inline-block max-w-[90%] whitespace-pre-wrap break-words rounded-md border border-line/70 bg-card px-2 py-1 text-left text-[13px] text-text">
                {m.text}
              </div>
            </div>
          );
        })}
        <div ref={endRef} />
      </div>

      <div className="shrink-0 border-t border-line p-2">
        {canPost ? (
          <textarea
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                send();
              }
            }}
            rows={1}
            placeholder="Message people on this board…"
            className="block w-full resize-none rounded-md border border-line bg-canvas px-2 py-1.5 text-[13px] text-bright outline-none placeholder:text-faint focus:border-accent"
          />
        ) : (
          <div className="text-[11.5px] text-faint">
            Read-only — you can see messages but not post.
          </div>
        )}
      </div>
    </div>
  );
}

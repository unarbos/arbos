import { useState } from "react";

/** "Alice is typing…" / "Alice and Bob are typing…" / "Several people…". */
function typingLine(names: string[]): string {
  if (names.length === 0) return "";
  if (names.length === 1) return `${names[0]} is typing…`;
  if (names.length === 2) return `${names[0]} and ${names[1]} are typing…`;
  return "Several people are typing…";
}

/**
 * The People panel: a human-to-human side chat for collaborators on this board.
 * It is NOT the agent conversation — messages here go to the other people on
 * the session, never to the model. Presentational: the parent (PeopleSurface)
 * owns the message state, presence, and the seam sends. Following the chat
 * convention, you see other participants' names but never your own.
 */
export function PeopleChat({
  selfName,
  canPost,
  roster,
  typing,
  onSend,
  onTyping,
}: {
  selfName: string;
  canPost: boolean;
  roster: string[];
  typing: string[];
  onSend: (text: string) => void;
  onTyping: () => void;
}) {
  const [draft, setDraft] = useState("");

  const send = () => {
    const t = draft.trim();
    if (!t) return;
    onSend(t);
    setDraft("");
  };

  // The roster and the typing line both show OTHER people only — never
  // yourself (you know you're here). Matches the "see others, not yourself"
  // message convention and avoids a confusing self/"Host" entry.
  const otherPeople = (roster ?? []).filter((n) => n !== selfName);
  const line = typingLine((typing ?? []).filter((n) => n !== selfName));

  return (
    <div className="flex h-full w-full flex-col bg-panel">
      {/* Online roster: the OTHER people currently on this board (not you). */}
      <div className="shrink-0 border-b border-line px-3 py-2">
        <div className="mb-1 text-[10.5px] uppercase tracking-wider text-faint select-none">
          Online · {otherPeople.length}
        </div>
        {otherPeople.length === 0 ? (
          <div className="text-[12px] text-faint">No one else here yet.</div>
        ) : (
          <div className="flex flex-wrap gap-1.5">
            {otherPeople.map((name) => (
              <span
                key={name}
                className="inline-flex items-center gap-1 rounded-full border border-line bg-card px-2 py-0.5 text-[11.5px] text-text"
              >
                <span className="size-1.5 rounded-full bg-green" />
                {name}
              </span>
            ))}
          </div>
        )}
      </div>

      {/* Side notes now fold inline into the conversation timeline (ADR-0038),
          so this panel is just presence + posting — the notes themselves show
          in the chat where they belong, not in a second list here. */}
      <div className="min-h-0 flex-1 overflow-y-auto px-3 py-2">
        <div className="text-[12px] leading-relaxed text-faint">
          Side chat for people on this board. What you post here appears inline
          in the conversation as a side note — the agent doesn’t see it.
        </div>
      </div>

      {/* Typing indicator: reserve a line so the composer doesn't jump, with
          breathing room above the input. */}
      <div className="min-h-[1.1rem] shrink-0 px-3 pt-0.5 pb-2 text-[11px] italic text-faint">
        {line}
      </div>

      <div className="shrink-0 border-t border-line p-2">
        {canPost ? (
          <textarea
            value={draft}
            onChange={(e) => {
              setDraft(e.target.value);
              onTyping();
            }}
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

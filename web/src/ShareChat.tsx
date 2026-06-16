import { useState } from "react";
import { X } from "lucide-react";

import { ChatTab } from "./components/ChatTab";
import { PeopleSurface } from "./components/PeopleSurface";
import type { SharePerm } from "./lib/api";
import type { Surface } from "./lib/surface";

/**
 * The share-mode view: a scoped session link drops the holder onto exactly one
 * chat, rendered by the REAL ChatTab — same transcript, same composer, same
 * live seam as the workspace — just without the surrounding tabs/history/
 * settings chrome, and read-only when the grant is read. There is no second
 * chat client: the seam is scoped server-side (the frame-filter locks it to
 * this session and gates writes), so the same component simply works under a
 * narrower permission. See ADR-0034.
 *
 * The People side chat is the same `PeopleSurface` the workspace opens as a
 * tab — here, with no tab system around us, it docks as a collapsible right
 * column the chat's People rail opens.
 */
export function ShareChat({ session, perm }: { session: string; perm: SharePerm }) {
  const canWrite = perm === "write" || perm === "admin";
  // The People panel a guest sees: opened by the chat's rail (onOpenSurface),
  // shown as a docked column. We hold the surface ref the chat hands us.
  const [people, setPeople] = useState<Surface | null>(null);
  return (
    <div className="flex h-dvh flex-col bg-canvas text-text">
      <header className="flex shrink-0 items-center gap-2 border-b border-line px-4 py-2.5">
        <span className="text-[13px] font-semibold text-bright">arbos</span>
        <span className="rounded-full border border-line px-2 py-0.5 text-[11px] text-muted">
          {canWrite ? "Shared conversation · you can reply" : "Shared conversation · read-only"}
        </span>
      </header>
      <div className="flex min-h-0 flex-1">
        <div className="flex min-h-0 flex-1 flex-col">
          <ChatTab
            active
            focused
            focusTick={0}
            resumeId={session}
            readOnly={!canWrite}
            sharedTab
            handle={{
              // The rail opens People; here we dock it instead of a tab. Only
              // the people surface is openable in this chrome-less view.
              onOpenSurface: (s) => {
                if (s.kind === "people") setPeople(s);
              },
            }}
          />
        </div>
        {people && (
          <div className="flex min-h-0 w-72 shrink-0 flex-col border-l border-line">
            <div className="flex shrink-0 items-center justify-between border-b border-line px-3 py-2">
              <span className="text-[12px] font-semibold text-bright">People</span>
              <button
                type="button"
                onClick={() => setPeople(null)}
                title="Collapse people chat"
                className="flex size-6 items-center justify-center rounded-md text-muted hover:bg-hover hover:text-text"
              >
                <X size={13} />
              </button>
            </div>
            <div className="min-h-0 flex-1">
              <PeopleSurface surface={people} readOnly={!canWrite} />
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

import { ChatTab } from "./components/ChatTab";
import type { SharePerm } from "./lib/api";

/**
 * The share-mode view: a scoped session link drops the holder onto exactly one
 * chat, rendered by the REAL ChatTab — same transcript, same composer, same
 * live seam as the workspace — just without the surrounding tabs/history/
 * settings chrome, and read-only when the grant is read. There is no second
 * chat client: the seam is scoped server-side (the frame-filter locks it to
 * this session and gates writes), so the same component simply works under a
 * narrower permission. See ADR-0034.
 */
export function ShareChat({ session, perm }: { session: string; perm: SharePerm }) {
  const canWrite = perm === "write" || perm === "admin";
  return (
    <div className="flex h-dvh flex-col bg-canvas text-text">
      <header className="flex shrink-0 items-center gap-2 border-b border-line px-4 py-2.5">
        <span className="text-[13px] font-semibold text-bright">arbos</span>
        <span className="rounded-full border border-line px-2 py-0.5 text-[11px] text-muted">
          {canWrite ? "Shared conversation · you can reply" : "Shared conversation · read-only"}
        </span>
      </header>
      <ChatTab
        active
        focused
        focusTick={0}
        resumeId={session}
        readOnly={!canWrite}
        handle={{}}
      />
    </div>
  );
}

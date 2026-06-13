import { useEffect, useState } from "react";
import { Check, Copy, Loader2, X } from "lucide-react";

import { shareArtifact, type ShareScope } from "@/lib/api";
import { useClipboard } from "@/lib/useClipboard";

// The link lifetimes the dialog offers, in seconds (0 = the server's cap, the
// "no expiry" option). A standing share is a standing exposure, so a bounded
// life is the default; "no expiry" is last and opt-in.
const TTLS: { label: string; seconds: number }[] = [
  { label: "1 hour", seconds: 3600 },
  { label: "1 day", seconds: 86400 },
  { label: "7 days", seconds: 604800 },
  { label: "No expiry", seconds: 0 },
];

/**
 * The one scoped-share dialog, reused by every tab-level Share doorway: it
 * mints a read-only link to a single artifact (a chat or a file) and shows it
 * for copying. Scope decides what is shared; the dialog is otherwise identical
 * for a chat and a canvas. On a loopback-only host the mint endpoint 404s and
 * the dialog explains why instead.
 */
export function ShareDialog({
  scope,
  label,
  onClose,
}: {
  scope: ShareScope;
  label: string;
  onClose: () => void;
}) {
  const [ttl, setTtl] = useState(86400);
  const [url, setUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const clip = useClipboard();

  // Escape closes, matching the app's other overlays.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  // Changing the lifetime invalidates an already-minted link (re-minting would
  // orphan it), so clear back to the create step.
  useEffect(() => {
    setUrl(null);
    clip.reset();
  }, [ttl, scope.kind, scope.ref]);

  const create = async () => {
    setBusy(true);
    setError(null);
    try {
      const link = await shareArtifact(scope, ttl);
      setUrl(link);
      await clip.copy(link);
    } catch {
      setError(
        "Sharing needs a remotely reachable arbos (a forest join or a non-loopback bind).",
      );
    } finally {
      setBusy(false);
    }
  };

  const noun = scope.kind === "session" ? "chat" : "artifact";

  return (
    <div
      className="fixed inset-0 z-[60] grid place-items-center bg-black/40 p-4"
      onMouseDown={onClose}
    >
      <div
        className="flex w-[26rem] max-w-[92vw] flex-col gap-3 rounded-xl border border-line bg-card p-4 shadow-2xl shadow-black/50"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="flex items-center gap-2">
          <div className="min-w-0 flex-1">
            <div className="text-[13px] font-semibold text-bright">Share this {noun}</div>
            <div className="truncate text-[11.5px] text-muted">{label}</div>
          </div>
          <button
            type="button"
            title="Close"
            onMouseDown={onClose}
            className="flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-md text-muted transition-colors hover:bg-hover hover:text-text"
          >
            <X size={13} />
          </button>
        </div>

        {error ? (
          <div className="text-[12px] text-muted">{error}</div>
        ) : (
          <>
            <label className="flex items-center justify-between gap-2 text-[12px] text-muted">
              Link expires
              <select
                value={ttl}
                onChange={(e) => setTtl(Number(e.target.value))}
                className="rounded-md border border-line bg-panel px-2 py-1 text-[12px] text-text outline-none focus:border-accent"
              >
                {TTLS.map((t) => (
                  <option key={t.seconds} value={t.seconds}>
                    {t.label}
                  </option>
                ))}
              </select>
            </label>

            {!url ? (
              <button
                type="button"
                onClick={() => void create()}
                disabled={busy}
                className="flex items-center justify-center gap-1.5 rounded-md bg-btn px-2 py-2 text-[12.5px] font-semibold text-canvas transition-colors hover:bg-bright disabled:opacity-60"
              >
                {busy ? <Loader2 size={13} className="animate-spin" /> : null}
                Create link
              </button>
            ) : (
              <div className="flex items-center gap-1.5">
                <input
                  readOnly
                  value={url}
                  onFocus={(e) => e.currentTarget.select()}
                  className="min-w-0 flex-1 rounded-md border border-line bg-canvas px-2 py-1.5 font-mono text-[11px] text-text outline-none"
                />
                <button
                  type="button"
                  title={clip.state === "ok" ? "Copied" : "Copy link"}
                  onClick={() => void clip.copy(url)}
                  className="flex size-8 shrink-0 items-center justify-center rounded-md text-muted transition-colors hover:bg-hover hover:text-text"
                >
                  {clip.state === "ok" ? <Check size={14} /> : <Copy size={14} />}
                </button>
              </div>
            )}

            <div className="text-[11.5px] text-muted">
              {clip.state === "blocked"
                ? "Clipboard is blocked here — select the link and copy it manually."
                : scope.kind === "session"
                  ? "Read-only link to this conversation, including the tool output it contains."
                  : "Read-only link to this artifact (and its sibling assets)."}
            </div>
          </>
        )}
      </div>
    </div>
  );
}

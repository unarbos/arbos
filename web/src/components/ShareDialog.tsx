import { useEffect, useState } from "react";
import { Check, Copy, Loader2, X } from "lucide-react";

import { shareArtifact, type SharePerm, type ShareScope } from "@/lib/api";
import { useClipboard } from "@/lib/useClipboard";

// The permission tiers offered per scope. A chat can be read-only or
// read+talk (the recipient can converse with the agent); a file artifact is
// read-only for now. Admin/no-tools tiers are staged server-side, so they are
// not offered here — the dialog never shows a permission the node won't honor.
const PERMS: Record<ShareScope["kind"], { value: SharePerm; label: string; hint: string }[]> = {
  session: [
    { value: "read", label: "Read", hint: "View the conversation only" },
    { value: "write", label: "Read + talk", hint: "View and send messages to the agent" },
  ],
  file: [{ value: "read", label: "Read", hint: "View this artifact only" }],
  trajectory: [{ value: "read", label: "Read", hint: "View the full debug log only" }],
  all: [{ value: "admin", label: "Full access", hint: "Full control of the agent — equivalent to logging in" }],
};

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
  // A chat can be shared two ways: as the live conversation (the real read /
  // read+talk link) or as a static trajectory snapshot — the full debug log
  // (every tool call, output, context, and the token totals) rendered at a
  // link, for "send me the trajectory so I can debug it". The choice only
  // exists for a chat; every other scope shares exactly one way.
  const isChat = scope.kind === "session";
  const [mode, setMode] = useState<"conversation" | "trajectory">("conversation");
  const effScope: ShareScope =
    isChat && mode === "trajectory" ? { kind: "trajectory", ref: scope.ref } : scope;
  const perms = PERMS[effScope.kind];
  const [ttl, setTtl] = useState(86400);
  const [perm, setPerm] = useState<SharePerm>(() => perms[0].value);
  const [url, setUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // Bumped on every copy so the check icon re-mounts and replays its pop,
  // even when the button was already in the "copied" state.
  const [pulse, setPulse] = useState(0);
  const clip = useClipboard();

  const copyLink = (link: string) => {
    setPulse((n) => n + 1);
    void clip.copy(link);
  };

  // Default the permission to the scope's first (safest) tier whenever the
  // target changes, so reopening on a different scope never carries a stale,
  // possibly-invalid permission (e.g. "write" onto a file). Switching a chat
  // between conversation and trajectory counts as a target change.
  useEffect(() => {
    setPerm(PERMS[effScope.kind][0].value);
  }, [effScope.kind]);

  // Escape closes, matching the app's other overlays.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  // Changing any input invalidates an already-minted link (re-minting would
  // orphan it), so clear back to the create step.
  useEffect(() => {
    setUrl(null);
    clip.reset();
  }, [ttl, perm, mode, scope.kind, scope.ref]);

  const create = async () => {
    setBusy(true);
    setError(null);
    try {
      const link = await shareArtifact(effScope, ttl, perm);
      setUrl(link);
      copyLink(link);
    } catch {
      setError(
        "Sharing needs a remotely reachable arbos (a forest join or a non-loopback bind).",
      );
    } finally {
      setBusy(false);
    }
  };

  const noun =
    effScope.kind === "trajectory"
      ? "trajectory"
      : scope.kind === "session"
        ? "chat"
        : scope.kind === "all"
          ? "agent"
          : "artifact";

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
            {isChat ? (
              <div className="flex flex-col gap-1">
                <span className="text-[12px] text-muted">Share</span>
                <div className="flex gap-1 rounded-md border border-line bg-panel p-0.5">
                  {(
                    [
                      { value: "conversation", label: "Conversation" },
                      { value: "trajectory", label: "Trajectory" },
                    ] as const
                  ).map((m) => (
                    <button
                      key={m.value}
                      type="button"
                      title={
                        m.value === "trajectory"
                          ? "A static snapshot of the full debug log — every message, tool call, output, and the token totals"
                          : "The live conversation behind the real chat UI"
                      }
                      onClick={() => setMode(m.value)}
                      className={`flex-1 rounded px-2 py-1 text-[12px] transition-colors ${
                        mode === m.value
                          ? "bg-btn font-semibold text-canvas"
                          : "text-muted hover:bg-hover hover:text-text"
                      }`}
                    >
                      {m.label}
                    </button>
                  ))}
                </div>
              </div>
            ) : null}
            {perms.length > 1 ? (
              <div className="flex flex-col gap-1">
                <span className="text-[12px] text-muted">Permission</span>
                <div className="flex gap-1 rounded-md border border-line bg-panel p-0.5">
                  {perms.map((p) => (
                    <button
                      key={p.value}
                      type="button"
                      title={p.hint}
                      onClick={() => setPerm(p.value)}
                      className={`flex-1 rounded px-2 py-1 text-[12px] transition-colors ${
                        perm === p.value
                          ? "bg-btn font-semibold text-canvas"
                          : "text-muted hover:bg-hover hover:text-text"
                      }`}
                    >
                      {p.label}
                    </button>
                  ))}
                </div>
              </div>
            ) : null}
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
                  title={clip.state === "ok" ? "Copied — click to copy again" : "Copy link"}
                  onClick={() => copyLink(url)}
                  className={`flex size-8 shrink-0 cursor-pointer items-center justify-center rounded-md transition-[background-color,color,transform] active:scale-90 ${
                    clip.state === "ok"
                      ? "bg-green/15 text-green hover:bg-green/25"
                      : "text-muted hover:bg-hover hover:text-text"
                  }`}
                >
                  {clip.state === "ok" ? (
                    <Check key={pulse} size={14} className="copy-pop" />
                  ) : (
                    <Copy size={14} />
                  )}
                </button>
              </div>
            )}

            <div className="text-[11.5px] text-muted">
              {clip.state === "blocked"
                ? "Clipboard is blocked here — select the link and copy it manually."
                : effScope.kind === "trajectory"
                  ? "A static, read-only snapshot of the full session: every message, tool call, output, the injected context, the turn config, and the token totals — for debugging. Secret-looking strings are best-effort redacted before sharing."
                  : scope.kind === "all"
                    ? "Full access to this agent — anyone with the link is logged in until it expires. Treat it like your own login."
                    : scope.kind === "session"
                      ? perm === "write"
                        ? "Anyone with this link can read the conversation and send messages to the agent (running real turns)."
                        : "Read-only link to this conversation, including the tool output it contains."
                      : "Read-only link to this artifact (and its sibling assets)."}
            </div>
          </>
        )}
      </div>
    </div>
  );
}

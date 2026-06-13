import { useState, type ComponentType } from "react";
import {
  ChevronDown,
  CloudOff,
  Database,
  Hourglass,
  Plug,
  RotateCcw,
  TriangleAlert,
  type LucideProps,
} from "lucide-react";

import type { ErrorCategory } from "@/lib/transcript";

/**
 * The chat's terminal-error presentation: instead of dumping the kernel's raw
 * "category: message" string in red, an error becomes a self-explaining card —
 * an icon and human title for what failed, one line of plain guidance, the raw
 * detail tucked behind a toggle, and a retry affordance when the turn can be
 * re-run. The card maps the kernel ErrorKind (and the rate-limit shape of a
 * provider failure) to a tone so a transient hiccup reads calmer than a real
 * fault.
 */
export interface ErrorCardProps {
  /** Raw error detail from the kernel or seam. */
  message: string;
  /** What the failure is attributed to (kernel ErrorKind or "connection"). */
  category: ErrorCategory;
  /** The kernel's hint that re-running the same prompt could succeed. */
  retryable: boolean;
  /** Re-send the last prompt; absent disables the retry button. */
  onRetry?: () => void;
  /** Whether a retry can be issued right now (seam connected, turn idle). */
  canRetry?: boolean;
}

type Tone = "warn" | "error";

interface Presentation {
  Icon: ComponentType<LucideProps>;
  title: string;
  /** One line of plain-language guidance under the title. */
  hint: string;
  tone: Tone;
}

// The provider presentation keys on the kernel's already-computed retryable
// bit rather than re-parsing the error string: a retryable provider failure is
// a transient rate-limit/overload the agent already retried and fell back over,
// so it reads as the calmer "busy" state; a non-retryable one (dead key, bad
// request) is a real fault to fix.
function present(category: ErrorCategory, retryable: boolean): Presentation {
  switch (category) {
    case "provider":
      return retryable
        ? {
            Icon: Hourglass,
            title: "The model is busy",
            hint: "The provider was overloaded or rate-limited. arbos retried and tried any fallback models before stopping — try again in a moment.",
            tone: "warn",
          }
        : {
            Icon: CloudOff,
            title: "Couldn't reach the model",
            hint: "The model provider rejected the request (often a bad or missing API key, or an unknown model). Check your provider settings.",
            tone: "error",
          };
    case "connection":
      return {
        Icon: Plug,
        title: "Connection interrupted",
        hint: "The link to the agent dropped. It reconnects on its own — resend once it's back.",
        tone: "warn",
      };
    case "history":
      return {
        Icon: Database,
        title: "Couldn't load this conversation",
        hint: "The session history failed to load, so the turn was stopped to avoid answering on a partial thread.",
        tone: "error",
      };
    case "persist":
      return {
        Icon: Database,
        title: "Couldn't save the conversation",
        hint: "A write to the durable log failed, so the turn stopped rather than drift from what's saved. Your last message may not have been recorded.",
        tone: "error",
      };
    case "internal":
      return {
        Icon: TriangleAlert,
        title: "Something went wrong",
        hint: "An unexpected internal error ended the turn. This one is on arbos — retrying may clear it.",
        tone: "error",
      };
    default: {
      // Exhaustive: a new ErrorCategory must add a branch above. The runtime
      // fallback keeps an unexpected wire value from rendering blank.
      const _exhaustive: never = category;
      void _exhaustive;
      return {
        Icon: TriangleAlert,
        title: "Error",
        hint: "The turn ended with an error.",
        tone: "error",
      };
    }
  }
}

const toneStyles: Record<Tone, { border: string; bg: string; icon: string }> = {
  warn: { border: "border-warn/30", bg: "bg-warn/[0.06]", icon: "text-warn" },
  error: { border: "border-red/30", bg: "bg-red/[0.06]", icon: "text-red" },
};

export function ErrorCard({
  message,
  category,
  retryable,
  onRetry,
  canRetry,
}: ErrorCardProps) {
  const [open, setOpen] = useState(false);
  const p = present(category, retryable);
  const s = toneStyles[p.tone];
  // The raw detail is only worth a toggle when it adds something the hint
  // doesn't already say; a bare "provider error" placeholder is just noise.
  const detail = message.trim();
  const hasDetail = detail !== "" && detail.toLowerCase() !== "provider error";

  return (
    <div
      className={`flex items-start gap-3 rounded-lg border ${s.border} ${s.bg} px-3.5 py-3`}
      role="alert"
    >
      <p.Icon size={16} className={`mt-0.5 shrink-0 ${s.icon}`} aria-hidden />
      <div className="min-w-0 flex-1">
        <div className="text-[13px] font-medium text-bright">{p.title}</div>
        <div className="mt-0.5 text-[12px] leading-relaxed text-muted">{p.hint}</div>

        {(hasDetail || (retryable && onRetry)) && (
          <div className="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1.5">
            {retryable && onRetry && (
              <button
                type="button"
                onClick={onRetry}
                disabled={!canRetry}
                className="inline-flex items-center gap-1.5 rounded-md border border-line/70 bg-card px-2 py-1 text-[12px] text-bright transition-colors hover:bg-panel disabled:cursor-not-allowed disabled:opacity-50"
                title={canRetry ? "Resend the last message" : "Reconnecting…"}
              >
                <RotateCcw size={12} />
                Retry
              </button>
            )}
            {hasDetail && (
              <button
                type="button"
                onClick={() => setOpen((v) => !v)}
                className="inline-flex items-center gap-1 text-[11.5px] text-faint hover:text-muted"
                aria-expanded={open}
              >
                <ChevronDown
                  size={12}
                  className={`transition-transform ${open ? "rotate-180" : ""}`}
                />
                {open ? "Hide details" : "Details"}
              </button>
            )}
          </div>
        )}

        {open && hasDetail && (
          <pre className="mt-2 max-h-48 overflow-auto whitespace-pre-wrap break-words rounded-md border border-line/50 bg-bar px-2.5 py-2 font-mono text-[11px] leading-relaxed text-muted">
            {detail}
          </pre>
        )}
      </div>
    </div>
  );
}

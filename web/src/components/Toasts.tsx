import { useEffect, useRef, useState } from "react";
import { CircleAlert, X } from "lucide-react";

import { onToast } from "@/lib/toast";

const TOAST_MS = 6000;

interface Toast {
  id: number;
  msg: string;
}

/** The toast stack (bottom-right): failed user actions that would otherwise
 *  be silent no-ops. Each auto-dismisses; the ✕ dismisses sooner. */
export function Toasts() {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const nextId = useRef(1);

  useEffect(
    () =>
      onToast((msg) => {
        const id = nextId.current++;
        setToasts((t) => [...t, { id, msg }]);
        window.setTimeout(
          () => setToasts((t) => t.filter((x) => x.id !== id)),
          TOAST_MS,
        );
      }),
    [],
  );

  if (toasts.length === 0) return null;
  return (
    <div className="pointer-events-none fixed bottom-4 right-4 z-50 flex w-80 flex-col gap-2">
      {toasts.map((t) => (
        <div
          key={t.id}
          className="pointer-events-auto flex items-start gap-2 rounded-md border border-red/40 bg-panel px-3 py-2 text-[12.5px] text-text shadow-lg"
        >
          <CircleAlert size={14} className="mt-0.5 shrink-0 text-red" />
          <span className="min-w-0 flex-1 break-words">{t.msg}</span>
          <button
            type="button"
            onClick={() => setToasts((s) => s.filter((x) => x.id !== t.id))}
            title="Dismiss"
            className="shrink-0 cursor-pointer text-faint transition-colors hover:text-text"
          >
            <X size={12} />
          </button>
        </div>
      ))}
    </div>
  );
}

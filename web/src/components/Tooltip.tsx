import type { ReactNode } from "react";

/**
 * A small styled tooltip that beats the native `title` delay and matches the
 * panel's theme. Wraps a single trigger; the bubble sits just past it on the
 * chosen side ("bottom" for toolbars hugging the top edge, "top" for the
 * composer hugging the bottom) and fades in on hover/focus. Purely CSS-driven
 * via group-hover so there's no JS state to manage.
 */
export function Tooltip({
  label,
  side = "bottom",
  children,
}: {
  label: string;
  side?: "top" | "bottom";
  children: ReactNode;
}) {
  const placement =
    side === "top" ? "bottom-full mb-1.5" : "top-full mt-1.5";
  return (
    <span className="group/tip relative inline-flex">
      {children}
      <span
        role="tooltip"
        className={`pointer-events-none absolute left-1/2 z-50 -translate-x-1/2 whitespace-nowrap rounded-md border border-line bg-card px-1.5 py-1 text-[11px] leading-none text-bright opacity-0 shadow-lg shadow-black/40 transition-opacity delay-200 duration-100 group-hover/tip:opacity-100 ${placement}`}
      >
        {label}
      </span>
    </span>
  );
}

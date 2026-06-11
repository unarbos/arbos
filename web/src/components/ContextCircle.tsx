/**
 * Context-window gauge: a small ring that fills as the live turn's prompt
 * eats into the model's context length. Cursor's composer dot — at a glance
 * how much headroom is left, with the exact count on hover.
 */

const SIZE = 15;
const STROKE = 2.5;
const RADIUS = (SIZE - STROKE) / 2;
const CIRCUMFERENCE = 2 * Math.PI * RADIUS;

function fmtTokens(n: number): string {
  if (n >= 1000) return `${(n / 1000).toFixed(n >= 100_000 ? 0 : 1)}k`;
  return String(n);
}

/** Arc color tracks pressure: calm accent → warn → red as the window fills. */
function arcColor(frac: number): string {
  if (frac >= 0.9) return "var(--color-red)";
  if (frac >= 0.75) return "var(--color-warn)";
  return "var(--color-accent)";
}

export function ContextCircle({ used, total }: { used: number; total: number }) {
  if (total <= 0 || used <= 0) return null;

  const frac = Math.min(1, used / total);
  const pct = Math.round(frac * 100);
  const offset = CIRCUMFERENCE * (1 - frac);
  const label = `Context: ${fmtTokens(used)} / ${fmtTokens(total)} (${pct}%)`;

  return (
    <span
      title={label}
      aria-label={label}
      className="flex shrink-0 cursor-default items-center"
    >
      <svg
        width={SIZE}
        height={SIZE}
        viewBox={`0 0 ${SIZE} ${SIZE}`}
        className="-rotate-90"
      >
        <circle
          cx={SIZE / 2}
          cy={SIZE / 2}
          r={RADIUS}
          fill="none"
          stroke="var(--color-line)"
          strokeWidth={STROKE}
        />
        <circle
          cx={SIZE / 2}
          cy={SIZE / 2}
          r={RADIUS}
          fill="none"
          stroke={arcColor(frac)}
          strokeWidth={STROKE}
          strokeLinecap="round"
          strokeDasharray={CIRCUMFERENCE}
          strokeDashoffset={offset}
          style={{ transition: "stroke-dashoffset 0.4s ease, stroke 0.4s ease" }}
        />
      </svg>
    </span>
  );
}

import { useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  formatTick,
  niceTicks,
  type Plot,
  type PlotPoint,
  type PlotSeries,
  type SeriesKind,
} from "../lib/plot";

/* ------------------------------------------------------------------ */
/* Chart — inline-SVG renderer for the canonical `Plot` shape.         */
/* Zero chart dependencies (the Cursor canvas approach): lines, dots,  */
/* and rects computed from points, colored entirely by the theme's     */
/* `--color-*` tokens so charts restyle live with the theme picker.    */
/* ------------------------------------------------------------------ */

const SERIES_COLORS = [
  "var(--color-accent)",
  "var(--color-green)",
  "var(--color-warn)",
  "var(--color-syntax-keyword)",
  "var(--color-syntax-string)",
  "var(--color-red)",
];

const PAD = { top: 10, right: 12, bottom: 30, left: 44 };
const DEFAULT_HEIGHT = 230;

function useContainerWidth(): [React.RefObject<HTMLDivElement | null>, number] {
  const ref = useRef<HTMLDivElement | null>(null);
  const [width, setWidth] = useState(0);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      setWidth(entries[0].contentRect.width);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return [ref, width];
}

type Scale = (v: number) => number;

function makeScale(d0: number, d1: number, r0: number, r1: number): Scale {
  const span = d1 - d0 || 1;
  return (v) => r0 + ((v - d0) / span) * (r1 - r0);
}

/** Data domain across series, padded; bars force the y-baseline to 0. */
function computeDomains(plot: Plot): { x: [number, number]; y: [number, number] } {
  let xMin = Infinity;
  let xMax = -Infinity;
  let yMin = Infinity;
  let yMax = -Infinity;
  for (const s of plot.series) {
    for (const p of s.points) {
      if (!Number.isFinite(p.x) || !Number.isFinite(p.y)) continue;
      if (p.x < xMin) xMin = p.x;
      if (p.x > xMax) xMax = p.x;
      if (p.y < yMin) yMin = p.y;
      if (p.y > yMax) yMax = p.y;
    }
  }
  if (xMin === Infinity) {
    xMin = 0;
    xMax = 1;
    yMin = 0;
    yMax = 1;
  }
  const hasBars = plot.series.some((s) => s.kind === "bar");
  if (hasBars) {
    yMin = Math.min(yMin, 0);
    yMax = Math.max(yMax, 0);
  }
  if (yMin === yMax) {
    yMin -= 1;
    yMax += 1;
  }
  const yPad = (yMax - yMin) * 0.06;
  const y: [number, number] = plot.yDomain ?? [
    hasBars && yMin === 0 ? 0 : yMin - yPad,
    yMax + yPad,
  ];
  let x: [number, number] = plot.xDomain ?? [xMin, xMax];
  if (hasBars && !plot.xDomain) {
    // Half a slot of breathing room so edge bars aren't clipped.
    const slot = minGap(plot) ?? 1;
    x = [x[0] - slot / 2, x[1] + slot / 2];
  }
  return { x, y };
}

/** Smallest gap between consecutive x values of any bar series. */
function minGap(plot: Plot): number | undefined {
  let gap: number | undefined;
  for (const s of plot.series) {
    if (s.kind !== "bar") continue;
    const xs = s.points
      .map((p) => p.x)
      .filter(Number.isFinite)
      .sort((a, b) => a - b);
    for (let i = 1; i < xs.length; i++) {
      const g = xs[i] - xs[i - 1];
      if (g > 0 && (gap === undefined || g < gap)) gap = g;
    }
  }
  return gap;
}

/** Polyline segments split at NaN gaps (function poles, missing data). */
function lineSegments(points: PlotPoint[], sx: Scale, sy: Scale): string[] {
  const paths: string[] = [];
  let d = "";
  for (const p of points) {
    if (!Number.isFinite(p.x) || !Number.isFinite(p.y)) {
      if (d) paths.push(d);
      d = "";
      continue;
    }
    d += `${d ? "L" : "M"}${sx(p.x).toFixed(2)},${sy(p.y).toFixed(2)}`;
  }
  if (d) paths.push(d);
  return paths;
}

type HoverState = {
  px: number; // pointer x in svg coords
  entries: { name: string; color: string; point: PlotPoint }[];
};

function nearestByX(series: PlotSeries, x: number): PlotPoint | undefined {
  let best: PlotPoint | undefined;
  let bestDist = Infinity;
  for (const p of series.points) {
    if (!Number.isFinite(p.x) || !Number.isFinite(p.y)) continue;
    const d = Math.abs(p.x - x);
    if (d < bestDist) {
      bestDist = d;
      best = p;
    }
  }
  return best;
}

export function Chart({ plot, height = DEFAULT_HEIGHT }: { plot: Plot; height?: number }) {
  const [ref, width] = useContainerWidth();
  const [hover, setHover] = useState<HoverState | null>(null);

  const layout = useMemo(() => {
    if (width <= 0) return null;
    const domains = computeDomains(plot);
    const innerW = Math.max(40, width - PAD.left - PAD.right);
    const innerH = Math.max(40, height - PAD.top - PAD.bottom);
    const sx = makeScale(domains.x[0], domains.x[1], PAD.left, PAD.left + innerW);
    const sy = makeScale(domains.y[0], domains.y[1], PAD.top + innerH, PAD.top);
    const xTicks =
      plot.xTicks?.filter((t) => t.value >= domains.x[0] && t.value <= domains.x[1]) ??
      niceTicks(domains.x[0], domains.x[1], Math.max(3, Math.floor(innerW / 70))).map(
        (value) => ({ value, label: formatTick(value) }),
      );
    const yTicks = niceTicks(domains.y[0], domains.y[1], Math.max(3, Math.floor(innerH / 40)));
    return { domains, innerW, innerH, sx, sy, xTicks, yTicks };
  }, [plot, width, height]);

  if (!layout) return <div ref={ref} style={{ height }} />;

  const { domains, innerH, sx, sy, xTicks, yTicks } = layout;
  const barSeries = plot.series.filter((s) => s.kind === "bar");
  const slotPx = barSeries.length
    ? Math.abs(sx((minGap(plot) ?? 1)) - sx(0)) * 0.72
    : 0;
  const barW = barSeries.length ? Math.max(2, slotPx / barSeries.length) : 0;
  const yZero = sy(Math.max(domains.y[0], Math.min(domains.y[1], 0)));

  const showLegend = plot.series.length > 1 || plot.series.some((s) => s.name);

  const onPointerMove = (e: React.PointerEvent<SVGSVGElement>) => {
    const rect = e.currentTarget.getBoundingClientRect();
    const px = e.clientX - rect.left;
    if (px < PAD.left || px > width - PAD.right) {
      setHover(null);
      return;
    }
    const dataX =
      domains.x[0] + ((px - PAD.left) / (width - PAD.left - PAD.right)) * (domains.x[1] - domains.x[0]);
    const entries: HoverState["entries"] = [];
    plot.series.forEach((s, i) => {
      const p = nearestByX(s, dataX);
      if (p) {
        entries.push({
          name: s.name ?? `series ${i + 1}`,
          color: SERIES_COLORS[i % SERIES_COLORS.length],
          point: p,
        });
      }
    });
    setHover(entries.length ? { px: sx(entries[0].point.x), entries } : null);
  };

  return (
    <div ref={ref} className="select-none">
      {plot.title && (
        <div className="px-1 pb-1 text-[12.5px] font-semibold text-bright">{plot.title}</div>
      )}
      <div className="relative">
        <svg
          width={width}
          height={height}
          onPointerMove={onPointerMove}
          onPointerLeave={() => setHover(null)}
        >
          {/* gridlines + y ticks */}
          {yTicks.map((v) => (
            <g key={`y${v}`}>
              <line
                x1={PAD.left}
                x2={width - PAD.right}
                y1={sy(v)}
                y2={sy(v)}
                stroke="var(--color-line)"
                strokeWidth={1}
              />
              <text
                x={PAD.left - 6}
                y={sy(v)}
                textAnchor="end"
                dominantBaseline="central"
                fill="var(--color-faint)"
                fontSize={10}
              >
                {formatTick(v)}
              </text>
            </g>
          ))}

          {/* x ticks */}
          {xTicks.map((t) => (
            <text
              key={`x${t.value}`}
              x={sx(t.value)}
              y={PAD.top + innerH + 14}
              textAnchor="middle"
              fill="var(--color-faint)"
              fontSize={10}
            >
              {t.label}
            </text>
          ))}

          {/* zero/base axis line */}
          <line
            x1={PAD.left}
            x2={width - PAD.right}
            y1={yZero}
            y2={yZero}
            stroke="var(--color-muted)"
            strokeOpacity={0.45}
            strokeWidth={1}
          />

          {/* series */}
          {plot.series.map((s, i) => {
            const color = SERIES_COLORS[i % SERIES_COLORS.length];
            switch (s.kind) {
              case "line":
                return (
                  <g key={i}>
                    {lineSegments(s.points, sx, sy).map((d, j) => (
                      <path
                        key={j}
                        d={d}
                        fill="none"
                        stroke={color}
                        strokeWidth={1.6}
                        strokeLinejoin="round"
                        strokeLinecap="round"
                      />
                    ))}
                  </g>
                );
              case "scatter":
                return (
                  <g key={i} fill={color}>
                    {s.points
                      .filter((p) => Number.isFinite(p.x) && Number.isFinite(p.y))
                      .map((p, j) => (
                        <circle key={j} cx={sx(p.x)} cy={sy(p.y)} r={2.6} />
                      ))}
                  </g>
                );
              case "bar": {
                const slot = barSeries.indexOf(s);
                const groupOffset = (slot - (barSeries.length - 1) / 2) * barW;
                return (
                  <g key={i} fill={color} fillOpacity={0.85}>
                    {s.points
                      .filter((p) => Number.isFinite(p.x) && Number.isFinite(p.y))
                      .map((p, j) => {
                        const yPix = sy(p.y);
                        const top = Math.min(yPix, yZero);
                        return (
                          <rect
                            key={j}
                            x={sx(p.x) + groupOffset - barW / 2 + 0.5}
                            y={top}
                            width={Math.max(1, barW - 1)}
                            height={Math.max(1, Math.abs(yZero - yPix))}
                            rx={1}
                          />
                        );
                      })}
                  </g>
                );
              }
              default: {
                const exhaustive: never = s.kind;
                return exhaustive;
              }
            }
          })}

          {/* hover guide */}
          {hover && (
            <line
              x1={hover.px}
              x2={hover.px}
              y1={PAD.top}
              y2={PAD.top + innerH}
              stroke="var(--color-muted)"
              strokeOpacity={0.5}
              strokeDasharray="3 3"
            />
          )}
        </svg>

        {hover && (
          <div
            className="pointer-events-none absolute top-1 z-10 rounded-md border border-line bg-card px-2 py-1.5 text-[11px] leading-relaxed"
            style={
              hover.px > width / 2
                ? { right: width - hover.px + 8 }
                : { left: hover.px + 8 }
            }
          >
            {hover.entries.map((e, i) => (
              <div key={i} className="flex items-center gap-1.5 whitespace-nowrap">
                <span className="h-1.5 w-1.5 rounded-full" style={{ background: e.color }} />
                <span className="text-muted">{e.name}</span>
                <span className="font-mono text-bright">
                  ({formatTick(e.point.x)}, {formatTick(e.point.y)})
                </span>
              </div>
            ))}
          </div>
        )}
      </div>

      <div className="flex items-baseline justify-between px-1 pt-0.5">
        <div className="flex flex-wrap gap-x-3 gap-y-0.5">
          {showLegend &&
            plot.series.map((s, i) => (
              <span key={i} className="flex items-center gap-1.5 text-[11px] text-muted">
                <SeriesGlyph kind={s.kind} color={SERIES_COLORS[i % SERIES_COLORS.length]} />
                {s.name ?? `series ${i + 1}`}
              </span>
            ))}
        </div>
        {(plot.xLabel || plot.yLabel) && (
          <span className="text-[10.5px] text-faint">
            {plot.yLabel && plot.xLabel
              ? `${plot.yLabel} vs ${plot.xLabel}`
              : (plot.yLabel ?? plot.xLabel)}
          </span>
        )}
      </div>
    </div>
  );
}

function SeriesGlyph({ kind, color }: { kind: SeriesKind; color: string }) {
  switch (kind) {
    case "line":
      return <span className="h-[2px] w-3 rounded-full" style={{ background: color }} />;
    case "scatter":
      return <span className="h-1.5 w-1.5 rounded-full" style={{ background: color }} />;
    case "bar":
      return <span className="h-2.5 w-1.5 rounded-[2px]" style={{ background: color }} />;
    default: {
      const exhaustive: never = kind;
      return exhaustive;
    }
  }
}

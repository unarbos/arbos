/* ------------------------------------------------------------------ */
/* plot.ts — the canonical chart data structure and everything that    */
/* normalizes into it. The renderer (Chart.tsx) consumes only `Plot`:  */
/* numeric points grouped into series. Convenience input forms (math   */
/* expressions, label-aligned value arrays) are compiled down to       */
/* points here, so the renderer never evaluates or guesses anything.   */
/* ------------------------------------------------------------------ */

export type PlotPoint = { x: number; y: number };

export type SeriesKind = "line" | "scatter" | "bar";

export type PlotSeries = {
  name?: string;
  kind: SeriesKind;
  /** y may be NaN to mark a gap (e.g. a function pole); renderers split there. */
  points: PlotPoint[];
};

export type PlotTick = { value: number; label: string };

export type Plot = {
  title?: string;
  xLabel?: string;
  yLabel?: string;
  xDomain?: [number, number];
  yDomain?: [number, number];
  /** Category labels mapped onto numeric x positions (from spec `labels`). */
  xTicks?: PlotTick[];
  series: PlotSeries[];
};

/* ------------------------------------------------------------------ */
/* Spec — the JSON shape the model emits inside a ```chart fence.      */
/* Loose on purpose: points as pairs or {x,y}, bare y arrays against   */
/* labels, or an expression with a domain.                             */
/* ------------------------------------------------------------------ */

type SpecPoint = [number, number] | { x: number; y: number };

type SpecSeries = {
  name?: string;
  kind?: string;
  points?: SpecPoint[];
  /** Values aligned by index with the plot-level `labels` (or 0..n-1). */
  y?: number[];
  /** Math expression in x, sampled over `domain`. */
  expr?: string;
  domain?: [number, number];
  samples?: number;
};

type Spec = {
  title?: string;
  xLabel?: string;
  yLabel?: string;
  labels?: string[];
  xDomain?: [number, number];
  yDomain?: [number, number];
  series?: SpecSeries[];
};

export type ParsePlotResult =
  | { ok: true; plot: Plot }
  | { ok: false; error: string };

const DEFAULT_SAMPLES = 200;
const MAX_SAMPLES = 2000;
/** Render budget across the whole plot — beyond it the fence falls back to JSON. */
const MAX_POINTS = 20_000;
const MAX_SERIES = 64;

function isPair(p: SpecPoint): p is [number, number] {
  return Array.isArray(p);
}

function asDomain(d: unknown): [number, number] | undefined {
  if (
    Array.isArray(d) &&
    d.length === 2 &&
    typeof d[0] === "number" &&
    typeof d[1] === "number" &&
    Number.isFinite(d[0]) &&
    Number.isFinite(d[1]) &&
    d[0] < d[1]
  ) {
    return [d[0], d[1]];
  }
  return undefined;
}

function normalizeKind(kind: string | undefined): SeriesKind | undefined {
  const k = kind ?? "line";
  return k === "line" || k === "scatter" || k === "bar" ? k : undefined;
}

function normalizeSeries(
  s: SpecSeries,
  i: number,
): { ok: true; series: PlotSeries } | { ok: false; error: string } {
  const kind = normalizeKind(s.kind);
  if (kind === undefined) {
    return {
      ok: false,
      error: `series ${i + 1}: unknown kind "${s.kind}" (use line, scatter, or bar)`,
    };
  }

  let points: PlotPoint[];

  if (s.points !== undefined) {
    if (!Array.isArray(s.points) || s.points.length === 0) {
      return { ok: false, error: `series ${i + 1}: "points" must be a non-empty array` };
    }
    points = [];
    for (const p of s.points) {
      const x = isPair(p) ? p[0] : p?.x;
      const y = isPair(p) ? p[1] : p?.y;
      if (typeof x !== "number" || typeof y !== "number") {
        return {
          ok: false,
          error: `series ${i + 1}: points must be [x, y] pairs or {x, y} objects`,
        };
      }
      points.push({ x, y });
    }
  } else if (s.y !== undefined) {
    if (!Array.isArray(s.y) || s.y.length === 0 || s.y.some((v) => typeof v !== "number")) {
      return { ok: false, error: `series ${i + 1}: "y" must be a non-empty number array` };
    }
    points = s.y.map((y, x) => ({ x, y }));
  } else if (typeof s.expr === "string") {
    const domain = asDomain(s.domain) ?? [-10, 10];
    let fn: (x: number) => number;
    try {
      fn = compileExpr(s.expr);
    } catch (e) {
      return {
        ok: false,
        error: `series ${i + 1}: ${e instanceof Error ? e.message : "bad expression"}`,
      };
    }
    const n = Math.min(Math.max(2, Math.floor(s.samples ?? DEFAULT_SAMPLES)), MAX_SAMPLES);
    points = [];
    for (let k = 0; k < n; k++) {
      const x = domain[0] + ((domain[1] - domain[0]) * k) / (n - 1);
      const y = fn(x);
      points.push({ x, y: Number.isFinite(y) ? y : NaN });
    }
    if (!points.some((p) => Number.isFinite(p.y))) {
      return { ok: false, error: `series ${i + 1}: "${s.expr}" produced no finite values` };
    }
  } else {
    return {
      ok: false,
      error: `series ${i + 1}: needs "points", "y", or "expr"`,
    };
  }

  return { ok: true, series: { name: s.name, kind, points } };
}

/**
 * Parse and normalize a ```chart fence body into the canonical `Plot`.
 * Never throws — malformed input (including mid-stream truncation)
 * returns `{ ok: false }` so callers can fall back to a code block.
 */
export function parsePlotSpec(source: string): ParsePlotResult {
  let spec: Spec;
  try {
    spec = JSON.parse(source) as Spec;
  } catch {
    return { ok: false, error: "invalid JSON" };
  }
  if (typeof spec !== "object" || spec === null) {
    return { ok: false, error: "spec must be a JSON object" };
  }
  if (!Array.isArray(spec.series) || spec.series.length === 0) {
    return { ok: false, error: `"series" must be a non-empty array` };
  }
  if (spec.series.length > MAX_SERIES) {
    return { ok: false, error: `too many series (max ${MAX_SERIES})` };
  }

  const labels = Array.isArray(spec.labels)
    ? spec.labels.filter((l): l is string => typeof l === "string")
    : undefined;

  const series: PlotSeries[] = [];
  let total = 0;
  for (let i = 0; i < spec.series.length; i++) {
    const r = normalizeSeries(spec.series[i], i);
    if (!r.ok) return r;
    total += r.series.points.length;
    if (total > MAX_POINTS) {
      return { ok: false, error: `too many points (max ${MAX_POINTS} across all series)` };
    }
    series.push(r.series);
  }

  return {
    ok: true,
    plot: {
      title: typeof spec.title === "string" ? spec.title : undefined,
      xLabel: typeof spec.xLabel === "string" ? spec.xLabel : undefined,
      yLabel: typeof spec.yLabel === "string" ? spec.yLabel : undefined,
      xDomain: asDomain(spec.xDomain),
      yDomain: asDomain(spec.yDomain),
      xTicks: labels?.length
        ? labels.map((label, value) => ({ value, label }))
        : undefined,
      series,
    },
  };
}

/* ------------------------------------------------------------------ */
/* Expression compiler — a small recursive-descent parser so function  */
/* plots never touch eval(). Grammar: + - * / % ^ (right-assoc),       */
/* unary minus, parens, implicit multiplication (2x, 3(x+1)),          */
/* one/two-arg functions, and the constants pi, e, tau.                */
/* ------------------------------------------------------------------ */

const FUNCS_1: Record<string, (a: number) => number> = {
  sin: Math.sin,
  cos: Math.cos,
  tan: Math.tan,
  asin: Math.asin,
  acos: Math.acos,
  atan: Math.atan,
  sinh: Math.sinh,
  cosh: Math.cosh,
  tanh: Math.tanh,
  sqrt: Math.sqrt,
  cbrt: Math.cbrt,
  abs: Math.abs,
  exp: Math.exp,
  ln: Math.log,
  log: Math.log,
  log10: Math.log10,
  log2: Math.log2,
  floor: Math.floor,
  ceil: Math.ceil,
  round: Math.round,
  sign: Math.sign,
};

const FUNCS_2: Record<string, (a: number, b: number) => number> = {
  min: Math.min,
  max: Math.max,
  pow: Math.pow,
  atan2: Math.atan2,
};

const CONSTS: Record<string, number> = {
  pi: Math.PI,
  e: Math.E,
  tau: Math.PI * 2,
};

type Token =
  | { type: "num"; value: number }
  | { type: "ident"; name: string }
  | { type: "op"; op: string };

function tokenize(src: string): Token[] {
  const tokens: Token[] = [];
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    if (c === " " || c === "\t" || c === "\n") {
      i++;
    } else if (/[0-9.]/.test(c)) {
      const m = src.slice(i).match(/^\d*\.?\d+(?:[eE][+-]?\d+)?/);
      if (!m) throw new Error(`bad number at "${src.slice(i, i + 8)}"`);
      tokens.push({ type: "num", value: parseFloat(m[0]) });
      i += m[0].length;
    } else if (/[a-zA-Z_]/.test(c)) {
      const m = src.slice(i).match(/^[a-zA-Z_][a-zA-Z_0-9]*/)!;
      tokens.push({ type: "ident", name: m[0] });
      i += m[0].length;
    } else if ("+-*/%^(),".includes(c)) {
      tokens.push({ type: "op", op: c });
      i++;
    } else {
      throw new Error(`unexpected character "${c}"`);
    }
  }
  return tokens;
}

type Evaluate = (x: number) => number;

/**
 * Compile a math expression in `x` to a plain function. Throws `Error`
 * with a human-readable message on any syntax problem.
 */
export function compileExpr(src: string): Evaluate {
  const tokens = tokenize(src);
  let pos = 0;

  const peek = () => tokens[pos];
  const isOp = (op: string) => {
    const t = peek();
    return t !== undefined && t.type === "op" && t.op === op;
  };
  const expect = (op: string) => {
    if (!isOp(op)) throw new Error(`expected "${op}"`);
    pos++;
  };

  function parseExpr(): Evaluate {
    let left = parseTerm();
    while (isOp("+") || isOp("-")) {
      const op = (tokens[pos] as { op: string }).op;
      pos++;
      const right = parseTerm();
      const l = left;
      left = op === "+" ? (x) => l(x) + right(x) : (x) => l(x) - right(x);
    }
    return left;
  }

  function parseTerm(): Evaluate {
    let left = parseUnary();
    for (;;) {
      let op: string | undefined;
      if (isOp("*") || isOp("/") || isOp("%")) {
        op = (tokens[pos] as { op: string }).op;
        pos++;
      } else {
        // Implicit multiplication: 2x, 2(x+1), x sin(x), (x+1)(x-1)
        const t = peek();
        if (t && (t.type === "num" || t.type === "ident" || (t.type === "op" && t.op === "("))) {
          op = "*";
        } else {
          break;
        }
      }
      const right = parseUnary();
      const l = left;
      left =
        op === "*"
          ? (x) => l(x) * right(x)
          : op === "/"
            ? (x) => l(x) / right(x)
            : (x) => l(x) % right(x);
    }
    return left;
  }

  function parseUnary(): Evaluate {
    if (isOp("-")) {
      pos++;
      const operand = parseUnary();
      return (x) => -operand(x);
    }
    if (isOp("+")) {
      pos++;
      return parseUnary();
    }
    return parsePower();
  }

  function parsePower(): Evaluate {
    const base = parseAtom();
    if (isOp("^")) {
      pos++;
      const exponent = parseUnary(); // right-associative, allows x^-2
      return (x) => Math.pow(base(x), exponent(x));
    }
    return base;
  }

  function parseAtom(): Evaluate {
    const t = peek();
    if (t === undefined) throw new Error("unexpected end of expression");

    if (t.type === "num") {
      pos++;
      const v = t.value;
      return () => v;
    }

    if (t.type === "op" && t.op === "(") {
      pos++;
      const inner = parseExpr();
      expect(")");
      return inner;
    }

    if (t.type === "ident") {
      pos++;
      const name = t.name;
      const isFunc = name in FUNCS_1 || name in FUNCS_2;
      // Only known functions consume a "(" as a call — x(x+1) and pi(x-1)
      // fall through to parseTerm's implicit multiplication instead.
      if (isFunc && isOp("(")) {
        pos++;
        const first = parseExpr();
        if (isOp(",")) {
          pos++;
          const second = parseExpr();
          expect(")");
          const fn2 = FUNCS_2[name];
          if (!fn2) throw new Error(`function "${name}" takes one argument`);
          return (x) => fn2(first(x), second(x));
        }
        expect(")");
        const fn1 = FUNCS_1[name];
        if (!fn1) throw new Error(`function "${name}" needs two arguments`);
        return (x) => fn1(first(x));
      }
      if (name === "x") return (x) => x;
      if (name in CONSTS) {
        const v = CONSTS[name];
        return () => v;
      }
      throw new Error(`unknown identifier "${name}" (only x, pi, e, tau)`);
    }

    throw new Error(`unexpected "${t.op}"`);
  }

  const fn = parseExpr();
  if (pos < tokens.length) {
    const t = tokens[pos];
    const what = t.type === "op" ? t.op : t.type === "ident" ? t.name : String(t.value);
    throw new Error(`unexpected "${what}"`);
  }
  return fn;
}

/* ------------------------------------------------------------------ */
/* Tick generation — "nice number" steps (1/2/5 × 10ⁿ), shared by both */
/* axes in the renderer.                                               */
/* ------------------------------------------------------------------ */

export function niceTicks(min: number, max: number, target = 5): number[] {
  if (!(max > min)) return [min];
  const span = max - min;
  const rawStep = span / Math.max(1, target);
  const mag = Math.pow(10, Math.floor(Math.log10(rawStep)));
  const norm = rawStep / mag;
  const step = (norm >= 5 ? 5 : norm >= 2 ? 2 : 1) * mag;
  const ticks: number[] = [];
  for (let v = Math.ceil(min / step) * step; v <= max + step * 1e-9; v += step) {
    // Snap floating-point drift (0.30000000000000004 → 0.3).
    ticks.push(Math.abs(v) < step * 1e-9 ? 0 : parseFloat(v.toPrecision(12)));
  }
  return ticks;
}

/** Compact tick label: 1500000 → "1.5M", 0.00012 → "1.2e-4". */
export function formatTick(v: number): string {
  if (v === 0) return "0";
  const abs = Math.abs(v);
  if (abs >= 1e9) return `${trimNum(v / 1e9)}B`;
  if (abs >= 1e6) return `${trimNum(v / 1e6)}M`;
  if (abs >= 1e4) return `${trimNum(v / 1e3)}k`;
  if (abs < 1e-3) return v.toExponential(1).replace("e-", "e-").replace(".0e", "e");
  return trimNum(v);
}

function trimNum(v: number): string {
  return String(parseFloat(v.toPrecision(6)));
}

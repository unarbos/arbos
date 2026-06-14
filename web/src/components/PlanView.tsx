import { Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";

import { fetchPlan, type Plan, type PlanNode } from "@/lib/api";
import { lazyPanel } from "@/lib/lazyPanel";
import { useTheme } from "@/lib/theme";
import type { Theme } from "@/lib/themes";
import { Highlight } from "./Markdown";

// The same diagram engine the chat's ```mermaid fences use — reused here so a
// plan renders in the app's palette with no second graph library. Lazy so the
// (large) mermaid bundle loads only when a plan is opened in Flow view.
const MermaidDiagram = lazyPanel(() => import("./Mermaid"));

const POLL_MS = 5000;

/** Dot colour per node status — the tree's at-a-glance health. */
function statusColor(status: string): string {
  switch (status) {
    case "done":
      return "bg-green";
    case "failed":
      return "bg-red";
    case "active":
      return "bg-accent";
    case "blocked":
      return "bg-muted";
    case "cancelled":
      return "bg-faint";
    default:
      return "bg-muted/50"; // pending
  }
}

/** A node arranged into the goal tree, depth carried for indentation. */
interface TreeNode {
  node: PlanNode;
  depth: number;
  children: TreeNode[];
}

/**
 * Rebuild the goal tree from the flat node list: children grouped under their
 * parent and ordered by seq, the root (node === plan) at depth 0.
 */
function buildTree(plan: Plan): TreeNode[] {
  const byParent = new Map<number, PlanNode[]>();
  for (const n of plan.nodes) {
    const key = n.node === plan.plan ? 0 : (n.parent ?? 0);
    const list = byParent.get(key) ?? [];
    list.push(n);
    byParent.set(key, list);
  }
  for (const list of byParent.values()) list.sort((a, b) => a.seq - b.seq);

  const walk = (parent: number, depth: number): TreeNode[] =>
    (byParent.get(parent) ?? []).map((n) => ({
      node: n,
      depth,
      children: walk(n.node, depth + 1),
    }));

  // The root is keyed under 0 alongside any orphans; its children hang off its
  // own id.
  return (byParent.get(0) ?? []).map((n) => ({
    node: n,
    depth: 0,
    children: walk(n.node, 1),
  }));
}

function flatten(tree: TreeNode[]): TreeNode[] {
  const out: TreeNode[] = [];
  const walk = (nodes: TreeNode[]) => {
    for (const t of nodes) {
      out.push(t);
      walk(t.children);
    }
  };
  walk(tree);
  return out;
}

/* ------------------------------------------------------------------ */
/* Flow view: the plan as a Mermaid flowchart — boxes per goal wired by */
/* the dependency the model authored (sibling order gates; same seq is  */
/* a parallel branch), the n8n-style "what runs, and in what order".    */
/* ------------------------------------------------------------------ */

/** First line of s, clipped to n runes with an ellipsis. */
function clipLine(s: string, n: number): string {
  const one = s.split("\n")[0] ?? "";
  return one.length > n ? one.slice(0, n - 1) + "…" : one;
}

/** Sanitize a string for use inside a quoted Mermaid node label. Mermaid's
 *  label parser is fragile: a literal double-quote closes the label and angle
 *  brackets collide with its HTML-label handling, so swap them for lookalikes
 *  rather than HTML entities (whose `;`/`&` confuse the lexer further). */
function mermaidEscape(s: string): string {
  return s
    .replace(/"/g, "'")
    .replace(/</g, "‹")
    .replace(/>/g, "›");
}

/** A recurring (standing) node never gates siblings; its `when` reads
 *  "every <period>". Used to keep maintain-nodes out of the gating chain. */
function isRecurring(n: PlanNode): boolean {
  return !!n.when && n.when.trim().toLowerCase().startsWith("every");
}

function nodeLabel(n: PlanNode): string {
  const lines = [`#${n.node} · ${n.executor.toUpperCase()}`, mermaidEscape(clipLine(n.goal, 44))];
  if (n.when) lines.push(`⏱ ${mermaidEscape(clipLine(n.when, 30))}`);
  const code = n.cmd ?? n.cond ?? n.notify;
  if (code) lines.push(`» ${mermaidEscape(clipLine(code, 40))}`);
  return lines.join("<br/>");
}

/** Per-executor node shape, so the diagram reads its kind at a glance:
 *  shell is a subroutine box, notify a stadium, ask a hexagon, agent a rect. */
function nodeShape(n: PlanNode): string {
  const id = `N${n.node}`;
  const label = nodeLabel(n);
  switch (n.executor) {
    // shell stays a plain rectangle: Mermaid's subroutine shape ([[ ]]) draws
    // an extra inner border that reads as an odd "double box".
    case "shell":
      return `${id}["${label}"]`;
    case "notify":
      return `${id}(["${label}"])`;
    case "ask":
      return `${id}{{"${label}"}}`;
    case "agent":
      return `${id}["${label}"]`;
    default: {
      const exhaustive: never = n.executor;
      void exhaustive;
      return `${id}["${label}"]`;
    }
  }
}

// Status → theme palette key for the node's outline. Concrete colors (not CSS
// vars) are inlined into classDef because Mermaid's classDef parser rejects
// `var(--…)` values.
const STATUS_COLOR_KEY: Record<string, keyof Theme["colors"]> = {
  done: "green",
  active: "accent",
  failed: "red",
  blocked: "warn",
  cancelled: "faint",
  pending: "muted",
};

/** The plan as a Mermaid `flowchart` source: a node per goal, decomposition
 *  and gating drawn as edges, status carried as a colored outline. */
function planToMermaid(plan: Plan, theme: Theme): string {
  const out: string[] = ["flowchart TD"];

  const byParent = new Map<number, PlanNode[]>();
  for (const n of plan.nodes) {
    const key = n.node === plan.plan ? 0 : (n.parent ?? 0);
    const arr = byParent.get(key);
    if (arr) arr.push(n);
    else byParent.set(key, [n]);
  }
  for (const arr of byParent.values()) arr.sort((a, b) => a.seq - b.seq);

  for (const n of plan.nodes) out.push("  " + nodeShape(n));

  // Group a sibling list into seq groups — same seq is a parallel group;
  // recurring siblings are excluded from the gating chain (they run alongside).
  const seqGroups = (arr: PlanNode[]): PlanNode[][] => {
    const groups: PlanNode[][] = [];
    for (const n of arr) {
      if (isRecurring(n)) continue;
      const last = groups[groups.length - 1];
      if (last && last[0].seq === n.seq) last.push(n);
      else groups.push([n]);
    }
    return groups;
  };

  const linkChildren = (parentId: number | null) => {
    const arr = byParent.get(parentId ?? 0) ?? [];
    const groups = seqGroups(arr);
    for (let i = 1; i < groups.length; i++) {
      for (const a of groups[i - 1]) {
        for (const b of groups[i]) out.push(`  N${a.node} --> N${b.node}`);
      }
    }
    if (parentId != null && groups.length > 0) {
      for (const b of groups[0]) out.push(`  N${parentId} --> N${b.node}`);
    }
    if (parentId != null) {
      for (const n of arr) {
        if (isRecurring(n)) out.push(`  N${parentId} -.-> N${n.node}`);
      }
    }
  };

  linkChildren(null);
  for (const n of plan.nodes) linkChildren(n.node);

  const byStatus = new Map<string, number[]>();
  for (const n of plan.nodes) {
    const arr = byStatus.get(n.status);
    if (arr) arr.push(n.node);
    else byStatus.set(n.status, [n.node]);
  }
  for (const [status, ids] of byStatus) {
    const key = STATUS_COLOR_KEY[status];
    const stroke = (key && theme.colors[key]) || theme.colors.line;
    out.push(`  classDef s_${status} stroke:${stroke},stroke-width:2px`);
    out.push(`  class ${ids.map((i) => "N" + i).join(",")} s_${status}`);
  }
  return out.join("\n");
}

function PlanFlow({ plan }: { plan: Plan }) {
  const theme = useTheme();
  const src = useMemo(() => planToMermaid(plan, theme), [plan, theme]);
  // No fence card here — the Preview is the whole panel, so the diagram fills
  // it directly (the renderer centers the SVG and caps it at the panel width).
  return (
    <Suspense
      fallback={
        <div className="px-3 py-2 text-[11.5px] text-faint">rendering diagram…</div>
      }
    >
      <MermaidDiagram content={src} />
    </Suspense>
  );
}

/* ------------------------------------------------------------------ */
/* Code view: the plan serialized as a YAML-ish document — the raw,     */
/* copyable "source" of the goal forest and each node's code.          */
/* ------------------------------------------------------------------ */

/** Render a scalar bare when it is unambiguous, JSON-quoted otherwise. */
function yamlScalar(s: string): string {
  if (s === "") return '""';
  if (/[:#]|^\s|\s$|^[[{>|*&!%@`"']/.test(s)) return JSON.stringify(s);
  return s;
}

/** A multi-line value as a YAML block scalar (`key: |`). */
function blockScalar(indent: string, key: string, body: string): string {
  const lines = body.split("\n").map((l) => `${indent}  ${l}`);
  return `${indent}${key}: |\n${lines.join("\n")}`;
}

function planToCode(plan: Plan): string {
  const out: string[] = [`# Plan #${plan.plan}`, `title: ${yamlScalar(plan.title)}`, "nodes:"];
  for (const { node: n } of flatten(buildTree(plan))) {
    out.push(`  - id: ${n.node}`);
    if (n.parent && n.node !== plan.plan) out.push(`    parent: ${n.parent}`);
    out.push(`    executor: ${n.executor}`);
    out.push(`    status: ${n.status}`);
    if (n.when) out.push(`    when: ${yamlScalar(n.when)}`);
    out.push(`    goal: ${yamlScalar(n.goal)}`);
    if (n.check) out.push(`    check: ${yamlScalar(n.check)}`);
    if (n.cmd) out.push(blockScalar("    ", "cmd", n.cmd));
    if (n.cond) out.push(blockScalar("    ", "cond", n.cond));
    if (n.notify) out.push(blockScalar("    ", "notify", n.notify));
    if (n.outcome) out.push(`    outcome: ${yamlScalar(n.outcome)}`);
  }
  return out.join("\n");
}

/** Raw view: the plan source rendered exactly like a read-only code file —
 *  headerless, highlighted per line, the file *is* the panel. */
function PlanCode({ plan }: { plan: Plan }) {
  const code = useMemo(() => planToCode(plan), [plan]);
  return (
    <pre className="overflow-x-auto px-4 py-3 font-mono text-[12px] leading-[1.55] text-text/90">
      {code.split("\n").map((line, i) => (
        <div key={i} className="whitespace-pre">
          {line ? <Highlight text={line} /> : " "}
        </div>
      ))}
    </pre>
  );
}

type PlanMode = "preview" | "raw" | "details";

const PLAN_MODES: { id: PlanMode; label: string }[] = [
  { id: "preview", label: "Preview" },
  { id: "raw", label: "Raw" },
  { id: "details", label: "Details" },
];

/** The segmented toggle, styled to match a markdown doc's Preview/Markdown
 *  pill: a quiet bordered control with no icons. */
function PlanModeToggle({
  mode,
  onChange,
}: {
  mode: PlanMode;
  onChange: (m: PlanMode) => void;
}) {
  return (
    <div className="flex items-center gap-0.5 rounded-md border border-line/60 bg-panel p-0.5">
      {PLAN_MODES.map(({ id, label }) => (
        <button
          key={id}
          type="button"
          onClick={() => onChange(id)}
          aria-pressed={mode === id}
          className={`cursor-pointer rounded px-2 py-0.5 text-[11.5px] transition-colors ${
            mode === id ? "bg-hover text-bright" : "text-muted hover:text-text"
          }`}
        >
          {label}
        </button>
      ))}
    </div>
  );
}

/**
 * The plan detail view: the whole goal forest a node belongs to, viewable as a
 * Mermaid flow graph, a raw code (YAML) serialization, or detail cards. Each
 * goal shows its definition and "code" — the shell command, gate predicate, or
 * notify payload — that discharges it. Opened by clicking a standing
 * obligation or a plan card; it answers "what exactly will the agent run?"
 */
export function PlanView({
  node,
  active,
}: {
  /** Any node in the plan; the view fetches its whole plan. */
  node: number;
  active: boolean;
}) {
  const [plan, setPlan] = useState<Plan | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [mode, setMode] = useState<PlanMode>("preview");

  const load = useCallback(() => {
    fetchPlan(node)
      .then((p) => {
        setPlan(p);
        setError(null);
      })
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)));
  }, [node]);

  // Re-poll while the tab is visible so a running plan's statuses stay live;
  // pause entirely when it's behind another tab.
  const loadRef = useRef(load);
  loadRef.current = load;
  useEffect(() => {
    loadRef.current();
    if (!active) return;
    const t = window.setInterval(() => loadRef.current(), POLL_MS);
    return () => window.clearInterval(t);
  }, [active, node]);

  const rows = plan ? flatten(buildTree(plan)) : [];

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <div className="flex h-9 shrink-0 select-none items-center border-b border-line/70 px-3">
        <PlanModeToggle mode={mode} onChange={setMode} />
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto">
        {error && plan === null ? (
          <div className="px-4 py-4 text-[12px] text-red">
            Failed to load plan: {error}
          </div>
        ) : plan === null ? (
          <div className="px-4 py-4 text-faint">Loading…</div>
        ) : plan.nodes.length === 0 ? (
          <div className="px-4 py-4 text-[12px] text-faint">
            This plan has no nodes.
          </div>
        ) : mode === "preview" ? (
          <PlanFlow plan={plan} />
        ) : mode === "raw" ? (
          <PlanCode plan={plan} />
        ) : (
          <div className="mx-auto w-full max-w-3xl space-y-2 px-4 py-4">
            {rows.map((t) => (
              <NodeCard key={t.node.node} node={t.node} depth={t.depth} />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

function NodeCard({ node, depth }: { node: PlanNode; depth: number }) {
  return (
    <div style={{ marginLeft: depth * 16 }}>
      <div className="rounded-md border border-line/60 px-3 py-2">
        <div className="flex items-center gap-2">
          <span
            className={`size-2 shrink-0 rounded-full ${statusColor(node.status)}`}
            title={node.status}
          />
          <span className="font-mono text-[11px] text-faint">#{node.node}</span>
          <span className="rounded bg-hover px-1.5 py-px text-[10.5px] uppercase tracking-wide text-muted">
            {node.executor}
          </span>
          <span className="text-[11px] text-faint">{node.status}</span>
          {node.when && (
            <span className="text-[11px] text-faint">· {node.when}</span>
          )}
        </div>

        <div className="mt-1.5 whitespace-pre-wrap break-words text-[12.5px] text-text">
          {node.goal}
        </div>

        {node.check && (
          <div className="mt-1 text-[11.5px] text-faint">
            <span className="text-muted">Check:</span> {node.check}
          </div>
        )}

        {node.cmd && <Code label="Command" body={node.cmd} />}
        {node.cond && <Code label="Gate (cond)" body={node.cond} />}
        {node.notify && <Code label="Notify" body={node.notify} />}

        {node.outcome && (
          <div className="mt-1.5 whitespace-pre-wrap break-words text-[11.5px] text-muted">
            <span className="text-faint">Outcome:</span> {node.outcome}
          </div>
        )}

        {node.attempts && node.attempts.length > 0 && (
          <Attempts attempts={node.attempts} />
        )}
      </div>
    </div>
  );
}

function Code({ label, body }: { label: string; body: string }) {
  return (
    <div className="mt-1.5">
      <div className="mb-0.5 text-[10.5px] uppercase tracking-wide text-faint">
        {label}
      </div>
      <pre className="overflow-x-auto rounded-md border border-line/60 bg-canvas/60 px-2.5 py-1.5 font-mono text-[11.5px] leading-relaxed text-text/90">
        {body}
      </pre>
    </div>
  );
}

function verdictColor(verdict: string): string {
  switch (verdict) {
    case "success":
      return "text-green";
    case "fail":
      return "text-red";
    default:
      return "text-muted";
  }
}

function Attempts({
  attempts,
}: {
  attempts: NonNullable<PlanNode["attempts"]>;
}) {
  const [open, setOpen] = useState(false);
  return (
    <div className="mt-1.5">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="cursor-pointer text-[11px] text-faint hover:text-text"
      >
        {open ? "Hide" : "Show"} {attempts.length} attempt
        {attempts.length === 1 ? "" : "s"}
      </button>
      {open && (
        <div className="mt-1 space-y-1 border-l border-line/60 pl-2.5">
          {attempts.map((a, i) => (
            <div key={i} className="text-[11.5px]">
              <span className={verdictColor(a.verdict)}>{a.verdict}</span>
              <span className="text-faint"> · {new Date(a.at).toLocaleString()}</span>
              {a.outcome && (
                <div className="whitespace-pre-wrap break-words text-muted">
                  {a.outcome}
                </div>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

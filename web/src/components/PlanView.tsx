import { useCallback, useEffect, useRef, useState } from "react";
import { Loader2, MessageSquare, RefreshCw } from "lucide-react";

import { fetchPlan, type Plan, type PlanNode } from "@/lib/api";

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

/**
 * The plan detail view: the whole goal forest a node belongs to, each goal
 * showing its definition and "code" — the shell command, gate predicate, or
 * notify payload that discharges it — plus its attempt history. Opened by
 * clicking a standing obligation; it answers "what exactly will the agent run?"
 */
export function PlanView({
  node,
  active,
  onOpenChat,
}: {
  /** Any node in the plan; the view fetches its whole plan. */
  node: number;
  active: boolean;
  onOpenChat?: (chat: string) => void;
}) {
  const [plan, setPlan] = useState<Plan | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const load = useCallback(() => {
    setLoading(true);
    fetchPlan(node)
      .then((p) => {
        setPlan(p);
        setError(null);
      })
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setLoading(false));
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
      <div className="flex shrink-0 items-center gap-2 border-b border-line/60 px-4 py-2">
        <div className="min-w-0 flex-1">
          <div className="truncate text-[13px] font-medium text-text">
            {plan?.title || `Plan #${plan?.plan ?? node}`}
          </div>
          {plan && (
            <div className="text-[11px] text-faint">
              Plan #{plan.plan} · {plan.nodes.length} node
              {plan.nodes.length === 1 ? "" : "s"}
            </div>
          )}
        </div>
        {plan?.chat && onOpenChat && (
          <button
            type="button"
            onClick={() => onOpenChat(plan.chat!)}
            title="Open owning chat"
            className="flex shrink-0 cursor-pointer items-center gap-1 rounded-md border border-line/60 px-2 py-1 text-[11.5px] text-muted transition-colors hover:bg-hover hover:text-text"
          >
            <MessageSquare size={12} />
            Chat
          </button>
        )}
        <button
          type="button"
          onClick={load}
          title="Refresh"
          className="flex size-7 shrink-0 cursor-pointer items-center justify-center rounded-md text-faint transition-colors hover:bg-hover hover:text-text"
        >
          {loading ? (
            <Loader2 size={13} className="animate-spin" />
          ) : (
            <RefreshCw size={13} />
          )}
        </button>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-3xl px-4 py-4">
          {error && plan === null ? (
            <div className="text-[12px] text-red">Failed to load plan: {error}</div>
          ) : plan === null ? (
            <div className="text-faint">Loading…</div>
          ) : rows.length === 0 ? (
            <div className="text-[12px] text-faint">This plan has no nodes.</div>
          ) : (
            <div className="space-y-2">
              {rows.map((t) => (
                <NodeCard key={t.node.node} node={t.node} depth={t.depth} />
              ))}
            </div>
          )}
        </div>
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

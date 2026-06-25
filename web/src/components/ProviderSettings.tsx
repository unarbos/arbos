import { useCallback, useEffect, useState } from "react";
import { Check, Pencil, Plus, RefreshCw, Trash2 } from "lucide-react";

import {
  activateProvider,
  addProvider,
  fetchLLM,
  fetchLLMCredits,
  removeProvider,
  resetModelsCache,
  updateProvider,
  type LLMCredits,
  type LLMInfo,
  type ProviderInput,
  type ProviderView,
} from "@/lib/api";

/**
 * The providers a user can add by name. Picking one fills in everything for
 * them — the entry's display name becomes the provider name and the endpoint
 * is prefilled (still editable) — so all that's left to type is the key.
 * `adapter` is the request shape the host builds ("openai" covers every
 * OpenAI-compatible endpoint: OpenRouter, vLLM, GM, OpenAI itself).
 */
const PROVIDER_PRESETS: {
  id: string;
  name: string;
  adapter: string;
  endpoint: string;
}[] = [
  {
    id: "openrouter",
    name: "OpenRouter",
    adapter: "openai",
    endpoint: "https://openrouter.ai/api/v1",
  },
  {
    id: "openai",
    name: "OpenAI",
    adapter: "openai",
    endpoint: "https://api.openai.com/v1",
  },
  {
    id: "google",
    name: "Google",
    adapter: "google",
    endpoint: "https://generativelanguage.googleapis.com",
  },
  {
    id: "anthropic",
    name: "Anthropic",
    adapter: "anthropic",
    endpoint: "https://api.anthropic.com",
  },
  {
    id: "vllm",
    name: "vLLM",
    adapter: "openai",
    endpoint: "http://localhost:8000/v1",
  },
  {
    id: "gm",
    name: "GM",
    adapter: "openai",
    endpoint: "https://api.saygm.com/v1",
  },
];

/** The request shapes (adapters) a provider can use, for the full edit form.
 * "openai" covers every OpenAI-compatible endpoint. */
const ADAPTER_OPTIONS = [
  { id: "openai", label: "OpenAI-compatible" },
  { id: "anthropic", label: "Anthropic" },
  { id: "google", label: "Google Gemini" },
];

/**
 * The Settings tab's provider panel: where the host's LLM requests go and the
 * key that signs them. The endpoint persists in the host preference file; the
 * key follows the secrets discipline — write-only, stored in the encrypted
 * vault, never returned. Saving either schedules a graceful host restart at
 * the next idle moment (the panel polls until the host is back), so the new
 * provider is rebuilt whole rather than hot-swapped. When the endpoint is
 * OpenRouter, the panel also shows the account's credit totals, fetched
 * through the host with the key attached server-side.
 */
export function ProviderSettings({ query }: { query: string }) {
  const [info, setInfo] = useState<LLMInfo | null>(null);
  const [credits, setCredits] = useState<LLMCredits | null>(null);

  // sync adopts a server picture and refreshes credits when the active
  // endpoint is OpenRouter. Called on mount and once an apply has landed.
  const sync = useCallback((i: LLMInfo) => {
    setInfo(i);
    if (i.openrouter && i.key_set) {
      fetchLLMCredits().then(setCredits).catch(() => setCredits(null));
    } else {
      setCredits(null);
    }
  }, []);

  useEffect(() => {
    fetchLLM()
      .then(sync)
      .catch(() => setInfo(null));
  }, [sync]);

  if (info === null) return null;

  const q = query.trim().toLowerCase();
  const matches =
    !q || "model provider endpoint api key credits openrouter".includes(q);
  if (!matches) return null;

  // One panel for everyone (ADR-0040 Option A): the host always reports at
  // least one provider — a real entry, or a single synthesized row from the
  // current single-provider config — so the list is the only view. A
  // single-provider host is just "a list of length 1" with a clear path to
  // add a second.
  return (
    <ProviderList
      info={info}
      credits={credits}
      onChanged={(fresh) => sync(fresh)}
      onRefreshCredits={() =>
        fetchLLMCredits().then(setCredits).catch(() => setCredits(null))
      }
    />
  );
}

/**
 * Multi-provider panel (ADR-0040): the configured providers as a list with an
 * active selector, add, and remove. Selecting/adding/removing schedules the
 * same graceful host restart as a single-provider save; this component polls
 * the boot id and adopts the rebuilt picture, mirroring the legacy flow.
 */
function ProviderList({
  info,
  credits,
  onChanged,
  onRefreshCredits,
}: {
  info: LLMInfo;
  credits: LLMCredits | null;
  onChanged: (fresh: LLMInfo) => void;
  onRefreshCredits: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);

  // Run a mutation, then poll until the host re-execs (new boot id) and adopt
  // the rebuilt configuration — the same applied-by-restart contract the
  // single-provider form uses.
  const applyThenSync = async (mutate: () => Promise<void>) => {
    const prevBoot = info.boot_id;
    setBusy(true);
    setError(null);
    try {
      await mutate();
    } catch (e) {
      setBusy(false);
      setError(e instanceof Error ? e.message : String(e));
      return false;
    }
    setStatus("Applying — restarting arbos…");
    try {
      for (let i = 0; i < 60; i++) {
        await new Promise((r) => setTimeout(r, 1500));
        try {
          const fresh = await fetchLLM();
          if (fresh.boot_id !== prevBoot) {
            resetModelsCache();
            onChanged(fresh);
            return true;
          }
          if (fresh.restart_pending) {
            setStatus("Saved — applying once the agent finishes its current work…");
          }
        } catch {
          // Host mid-restart; keep polling.
        }
      }
      setError("Saved, but the restart hasn't landed yet — it applies at the next idle moment.");
      return false;
    } finally {
      setBusy(false);
      setStatus(null);
      setAdding(false);
    }
  };

  const providers = info.providers ?? [];
  return (
    <div className="mb-6">
      <div className="mb-2 px-1 text-[12px] text-muted select-none">
        Model Providers
      </div>
      <div className="overflow-hidden rounded-xl bg-card/50">
        <p className="border-b border-line/30 px-4 py-2.5 text-[11.5px] leading-relaxed text-faint">
          Configure several providers and pick the active one. Switching
          restarts arbos at its next idle moment — this page reconnects by
          itself. Keys are encrypted on this machine and never shown again.
        </p>

        {error && (
          <div className="border-b border-line/30 px-4 py-2 text-[11.5px] text-red">
            {error}
          </div>
        )}

        <div className="flex flex-col">
          {providers.map((p) => (
            <ProviderRow
              key={p.id}
              p={p}
              active={p.id === info.active_provider_id}
              busy={busy}
              onActivate={() => void applyThenSync(() => activateProvider(p.id))}
              onRemove={() => void applyThenSync(() => removeProvider(p.id))}
              onRename={(name) =>
                void applyThenSync(() =>
                  updateProvider(p.id, {
                    label: name,
                    provider_name: p.provider_name,
                    endpoint: p.endpoint,
                    default_model: "",
                    key: null,
                  }),
                )
              }
              onSaveDetails={(d) =>
                void applyThenSync(() =>
                  updateProvider(p.id, {
                    label: p.label,
                    provider_name: d.providerName,
                    endpoint: d.endpoint,
                    default_model: "",
                    key: d.key,
                  }),
                )
              }
            />
          ))}
        </div>

        <div className="border-t border-line/30 px-4 py-2.5">
          {status ? (
            <span className="text-[11.5px] text-faint">{status}</span>
          ) : adding ? (
            <AddProviderForm
              busy={busy}
              onCancel={() => setAdding(false)}
              onSubmit={(p) => void applyThenSync(() => addProvider(p))}
            />
          ) : (
            <button
              type="button"
              onClick={() => setAdding(true)}
              disabled={busy}
              className="flex cursor-pointer items-center gap-1 rounded-md px-2 py-1 text-[12px] text-accent hover:bg-hover disabled:opacity-40"
            >
              <Plus size={13} /> Add provider
            </button>
          )}
        </div>

        {info.openrouter && info.key_set && (
          <CreditsRow credits={credits} onRefresh={onRefreshCredits} />
        )}
      </div>
    </div>
  );
}

/** One row in the provider list. Two edit affordances (hidden for env-sourced
 * read-only entries): double-click the name to rename it inline, or click the
 * pencil to expand the full edit form (request shape, endpoint, key). The name
 * is the entry's free-text label — defaulted to the provider's name on add. */
function ProviderRow({
  p,
  active,
  busy,
  onActivate,
  onRemove,
  onRename,
  onSaveDetails,
}: {
  p: ProviderView;
  active: boolean;
  busy: boolean;
  onActivate: () => void;
  onRemove: () => void;
  onRename: (name: string) => void;
  onSaveDetails: (d: { providerName: string; endpoint: string; key: string | null }) => void;
}) {
  const [editingName, setEditingName] = useState(false);
  const [name, setName] = useState(p.label);
  const [expanded, setExpanded] = useState(false);

  const commitName = () => {
    const next = name.trim();
    setEditingName(false);
    if (next !== "" && next !== p.label) onRename(next);
    else setName(p.label);
  };

  return (
    <div className="border-b border-line/20 last:border-b-0">
      <div className="flex items-center gap-3 px-4 py-2.5">
        <input
          type="radio"
          checked={active}
          disabled={busy}
          onChange={onActivate}
          aria-label={`Activate ${p.label}`}
          className="cursor-pointer accent-green"
        />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2 text-[12px] text-bright">
            {editingName ? (
              <input
                autoFocus
                value={name}
                onChange={(e) => setName(e.target.value)}
                onBlur={commitName}
                onKeyDown={(e) => {
                  if (e.key === "Enter") commitName();
                  if (e.key === "Escape") {
                    setName(p.label);
                    setEditingName(false);
                  }
                }}
                aria-label="Provider name"
                className="min-w-0 flex-1 rounded border border-line bg-panel px-1 py-0.5 text-[12px] text-bright outline-none"
              />
            ) : (
              <span
                onDoubleClick={() => {
                  if (p.env_sourced || busy) return;
                  setName(p.label);
                  setEditingName(true);
                }}
                title={p.env_sourced ? undefined : "Double-click to rename"}
                className={`truncate ${p.env_sourced ? "" : "cursor-text"}`}
              >
                {p.label}
              </span>
            )}
            {active && <span className="shrink-0 text-[10px] text-green">active</span>}
            {p.env_sourced && (
              <span className="shrink-0 text-[10px] text-faint">env</span>
            )}
          </div>
          <div className="mt-0.5 truncate font-mono text-[11px] text-faint">
            {p.endpoint || "(adapter default)"}
            {" · "}
            {p.key_set ? (
              <span className="text-green">key set</span>
            ) : (
              <span className="text-warn">no key</span>
            )}
          </div>
        </div>
        {!p.env_sourced && (
          <button
            type="button"
            aria-label={`Edit ${p.label}`}
            title="Edit endpoint & key"
            disabled={busy}
            onClick={() => setExpanded((v) => !v)}
            className={`flex size-7 shrink-0 cursor-pointer items-center justify-center rounded-md hover:bg-hover hover:text-bright disabled:opacity-40 ${
              expanded ? "text-bright" : "text-muted"
            }`}
          >
            <Pencil size={13} />
          </button>
        )}
        {!p.env_sourced && (
          <button
            type="button"
            aria-label={`Remove ${p.label}`}
            title="Remove provider"
            disabled={busy}
            onClick={onRemove}
            className="flex size-7 shrink-0 cursor-pointer items-center justify-center rounded-md text-muted hover:bg-hover hover:text-red disabled:opacity-40"
          >
            <Trash2 size={13} />
          </button>
        )}
      </div>
      {expanded && !p.env_sourced && (
        <ProviderEditForm
          p={p}
          busy={busy}
          onCancel={() => setExpanded(false)}
          onSave={(d) => {
            setExpanded(false);
            onSaveDetails(d);
          }}
        />
      )}
    </div>
  );
}

/** The inline full-edit form a row expands into from the pencil: change the
 * request shape, the endpoint, and the key (left blank to keep the stored one).
 * The name is renamed separately, by double-clicking it in the row. */
function ProviderEditForm({
  p,
  busy,
  onCancel,
  onSave,
}: {
  p: ProviderView;
  busy: boolean;
  onCancel: () => void;
  onSave: (d: { providerName: string; endpoint: string; key: string | null }) => void;
}) {
  const [providerName, setProviderName] = useState(p.provider_name);
  const [endpoint, setEndpoint] = useState(p.endpoint);
  const [key, setKey] = useState("");
  const canSave = !busy && endpoint.trim() !== "";

  const inputCls =
    "w-full rounded-md border border-line bg-panel px-2 py-1 font-mono text-[12px] text-bright outline-none placeholder:text-faint";
  return (
    <div className="flex flex-col gap-2 px-4 pt-1 pb-3">
      <select
        value={providerName}
        onChange={(e) => setProviderName(e.target.value)}
        className="w-full cursor-pointer rounded-md border border-line bg-panel px-2 py-1 text-[12px] text-bright outline-none"
      >
        {ADAPTER_OPTIONS.map((a) => (
          <option key={a.id} value={a.id}>
            {a.label}
          </option>
        ))}
      </select>
      <input
        value={endpoint}
        onChange={(e) => setEndpoint(e.target.value)}
        placeholder="Endpoint"
        spellCheck={false}
        className={inputCls}
      />
      <input
        type="password"
        value={key}
        onChange={(e) => setKey(e.target.value)}
        placeholder={p.key_set ? "API key (leave blank to keep current)" : "API key"}
        autoComplete="off"
        className={inputCls}
      />
      <div className="flex items-center justify-end gap-2">
        <button
          type="button"
          onClick={onCancel}
          disabled={busy}
          className="cursor-pointer rounded-md px-2.5 py-1 text-[12px] text-muted hover:bg-hover disabled:opacity-40"
        >
          Cancel
        </button>
        <button
          type="button"
          disabled={!canSave}
          onClick={() =>
            onSave({
              providerName,
              endpoint: endpoint.trim(),
              key: key.trim() === "" ? null : key.trim(),
            })
          }
          className="flex cursor-pointer items-center gap-1 rounded-md bg-green/90 px-2.5 py-1 text-[12px] text-white hover:bg-green disabled:cursor-default disabled:opacity-40"
        >
          <Check size={13} /> Save &amp; apply
        </button>
      </div>
    </div>
  );
}

/**
 * The inline form for adding a provider. The user just picks a provider by
 * name (OpenRouter, OpenAI, …) and we do the rest: the entry is named after
 * the provider, its adapter is chosen, and its endpoint is prefilled. Only the
 * endpoint stays editable (for self-hosted bases like vLLM) and the key is the
 * one thing we have to ask for.
 */
function AddProviderForm({
  busy,
  onCancel,
  onSubmit,
}: {
  busy: boolean;
  onCancel: () => void;
  onSubmit: (p: ProviderInput) => void;
}) {
  const [presetId, setPresetId] = useState(PROVIDER_PRESETS[0].id);
  const preset =
    PROVIDER_PRESETS.find((p) => p.id === presetId) ?? PROVIDER_PRESETS[0];
  // Endpoint follows the picked provider but stays editable. `null` means
  // "untouched, mirror the preset"; once the user edits it we keep their text.
  const [endpointEdit, setEndpointEdit] = useState<string | null>(null);
  const endpoint = endpointEdit ?? preset.endpoint;
  const [key, setKey] = useState("");
  const canAdd = !busy && endpoint.trim() !== "";

  const inputCls =
    "w-full rounded-md border border-line bg-panel px-2 py-1 font-mono text-[12px] text-bright outline-none placeholder:text-faint";
  return (
    <div className="flex flex-col gap-2">
      <select
        value={presetId}
        onChange={(e) => {
          setPresetId(e.target.value);
          setEndpointEdit(null);
        }}
        className="w-full cursor-pointer rounded-md border border-line bg-panel px-2 py-1 text-[12px] text-bright outline-none"
      >
        {PROVIDER_PRESETS.map((p) => (
          <option key={p.id} value={p.id}>
            {p.name}
          </option>
        ))}
      </select>
      <input
        value={endpoint}
        onChange={(e) => setEndpointEdit(e.target.value)}
        placeholder="Endpoint"
        spellCheck={false}
        className={inputCls}
      />
      <input
        type="password"
        value={key}
        onChange={(e) => setKey(e.target.value)}
        placeholder="API key"
        autoComplete="off"
        className={inputCls}
      />
      <div className="flex items-center justify-end gap-2">
        <button
          type="button"
          onClick={onCancel}
          disabled={busy}
          className="cursor-pointer rounded-md px-2.5 py-1 text-[12px] text-muted hover:bg-hover disabled:opacity-40"
        >
          Cancel
        </button>
        <button
          type="button"
          disabled={!canAdd}
          onClick={() =>
            onSubmit({
              label: preset.name,
              provider_name: preset.adapter,
              endpoint: endpoint.trim(),
              default_model: "",
              key: key.trim() === "" ? null : key.trim(),
            })
          }
          className="flex cursor-pointer items-center gap-1 rounded-md bg-green/90 px-2.5 py-1 text-[12px] text-white hover:bg-green disabled:cursor-default disabled:opacity-40"
        >
          <Check size={13} /> Add &amp; apply
        </button>
      </div>
    </div>
  );
}

/** OpenRouter account totals: used vs purchased, with the remainder. */
function CreditsRow({
  credits,
  onRefresh,
}: {
  credits: LLMCredits | null;
  onRefresh: () => void;
}) {
  const usd = (n: number) =>
    n.toLocaleString("en-US", { style: "currency", currency: "USD" });
  return (
    <div className="border-t border-line/30 px-4 py-3">
      <div className="flex items-center justify-between gap-3">
        <div className="min-w-0">
          <div className="text-[12px] text-bright">OpenRouter credits</div>
          {credits ? (
            <div className="mt-0.5 text-[11.5px] text-muted">
              {usd(Math.max(0, credits.total_credits - credits.total_usage))}{" "}
              left — {usd(credits.total_usage)} used of{" "}
              {usd(credits.total_credits)} purchased
            </div>
          ) : (
            <div className="mt-0.5 text-[11.5px] text-faint">
              Credits unavailable right now.
            </div>
          )}
        </div>
        <button
          type="button"
          aria-label="Refresh credits"
          title="Refresh credits"
          onClick={onRefresh}
          className="flex size-7 shrink-0 cursor-pointer items-center justify-center rounded-md text-muted hover:bg-hover hover:text-bright"
        >
          <RefreshCw size={13} />
        </button>
      </div>
      {credits && credits.total_credits > 0 && (
        <div className="mt-2 h-1.5 overflow-hidden rounded-full bg-panel">
          <div
            className="h-full rounded-full bg-green/80"
            style={{
              width: `${Math.min(
                100,
                (credits.total_usage / credits.total_credits) * 100,
              ).toFixed(1)}%`,
            }}
          />
        </div>
      )}
    </div>
  );
}

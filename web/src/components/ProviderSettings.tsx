import { useCallback, useEffect, useRef, useState } from "react";
import { Check, Plus, RefreshCw, Trash2 } from "lucide-react";

import {
  activateProvider,
  addProvider,
  fetchLLM,
  fetchLLMCredits,
  removeProvider,
  resetModelsCache,
  saveLLM,
  type LLMCredits,
  type LLMInfo,
  type ProviderInput,
  type ProviderView,
} from "@/lib/api";

const ADAPTERS = [
  { id: "openai", label: "OpenAI-compatible (OpenAI, OpenRouter, gm, vLLM…)" },
  { id: "anthropic", label: "Anthropic (native)" },
  { id: "google", label: "Google Gemini (native)" },
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
  const [endpoint, setEndpoint] = useState("");
  const [key, setKey] = useState("");
  const [credits, setCredits] = useState<LLMCredits | null>(null);
  const [busy, setBusy] = useState(false);
  const [applying, setApplying] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // The endpoint the loaded info reported, to detect an actual edit.
  const loadedEndpoint = useRef("");

  // sync adopts a server picture into the form — only called on mount and
  // once an apply has provably landed, so it never clobbers mid-edit or
  // mid-apply input with stale pre-restart values.
  const sync = useCallback((i: LLMInfo) => {
    setInfo(i);
    setEndpoint(i.endpoint);
    loadedEndpoint.current = i.endpoint;
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

  // Multi-provider mode: once the host reports a providers[] list, the panel
  // renders the list + add form instead of the single endpoint/key form
  // (ADR-0040). A host that never used the new panel reports no list and keeps
  // the original single-provider form below.
  if (info.providers && info.providers.length > 0) {
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

  const endpointChanged = endpoint.trim() !== loadedEndpoint.current;
  const canSave = !busy && !applying && (endpointChanged || key.trim() !== "");

  // The host applies a save by re-execing at its next idle moment, so a
  // successful fetch right after saving may still be the OLD process (or a
  // restart parked behind a busy agent). The boot id is the proof: poll until
  // it changes, surfacing the pending state, before adopting what the server
  // reports — otherwise the form would snap back to the old endpoint and the
  // save would look like it never happened.
  const save = async () => {
    const prevBoot = info.boot_id;
    // Whether this save is the first time a provider gets configured — the
    // onboarding case, where the catalog and the launch model were the
    // keyless fakes and the whole app should come up fresh on the real one.
    const wasUnconfigured = !info.key_set;
    setBusy(true);
    setError(null);
    try {
      await saveLLM({
        ...(endpointChanged ? { endpoint: endpoint.trim() } : {}),
        ...(key.trim() !== "" ? { key: key.trim() } : {}),
      });
    } catch (e) {
      setBusy(false);
      setError(e instanceof Error ? e.message : String(e));
      return;
    }
    setKey("");
    setBusy(false);
    setApplying(true);
    setStatus("Applying — restarting arbos…");
    try {
      for (let i = 0; i < 60; i++) {
        await new Promise((r) => setTimeout(r, 1500));
        try {
          const fresh = await fetchLLM();
          if (fresh.boot_id !== prevBoot) {
            // The new host is up with a real models endpoint where it had
            // none — drop the keyless catalog the page cached at load so
            // every picker fetches the provider's models on next open.
            resetModelsCache();
            if (fresh.key_set && wasUnconfigured) {
              // Onboarding: reboot the SPA so the model chip, the default
              // model, and the composer all come up on the configured
              // provider at once, with nothing of value to lose yet.
              window.location.reload();
              return;
            }
            sync(fresh);
            return;
          }
          if (fresh.restart_pending) {
            setStatus("Saved — applying once the agent finishes its current work…");
          }
        } catch {
          // Host mid-restart; keep polling.
        }
      }
      setError(
        "Saved, but the restart hasn't landed yet — it applies at the next idle moment.",
      );
    } finally {
      setApplying(false);
      setStatus(null);
    }
  };

  return (
    <div className="mb-6">
      <div className="mb-2 px-1 text-[12px] text-muted select-none">
        Model Provider
      </div>
      <div className="overflow-hidden rounded-xl bg-card/50">
        <p className="border-b border-line/30 px-4 py-2.5 text-[11.5px] leading-relaxed text-faint">
          Where the agent's LLM requests go. The key is encrypted on this
          machine and never shown again. Saving restarts arbos at its next
          idle moment — this page reconnects by itself.
        </p>

        {error && (
          <div className="border-b border-line/30 px-4 py-2 text-[11.5px] text-red">
            {error}
          </div>
        )}

        <div className="flex flex-col gap-2.5 px-4 py-3.5">
          <label className="flex flex-col gap-1">
            <span className="text-[11px] text-faint">Endpoint</span>
            <input
              value={endpoint}
              onChange={(e) => setEndpoint(e.target.value)}
              placeholder="https://openrouter.ai/api/v1"
              spellCheck={false}
              className="w-full rounded-md border border-line bg-panel px-2 py-1 font-mono text-[12px] text-bright outline-none placeholder:text-faint"
            />
          </label>

          <label className="flex flex-col gap-1">
            <span className="text-[11px] text-faint">
              API key{" "}
              {info.key_set && (
                <span className="text-green">— configured</span>
              )}
            </span>
            <input
              type="password"
              value={key}
              onChange={(e) => setKey(e.target.value)}
              placeholder={
                info.key_set ? "•••••••• (unchanged)" : "sk-or-… (openrouter.ai/keys)"
              }
              autoComplete="off"
              className="w-full rounded-md border border-line bg-panel px-2 py-1 font-mono text-[12px] text-bright outline-none placeholder:text-faint"
            />
          </label>

          <div className="mt-0.5 flex items-center justify-between gap-2">
            <span className="text-[11.5px] text-faint">
              {applying && status ? status : `Active model: ${info.model}`}
            </span>
            <button
              type="button"
              onClick={() => void save()}
              disabled={!canSave}
              className="flex cursor-pointer items-center gap-1 rounded-md bg-green/90 px-2.5 py-1 text-[12px] text-white hover:bg-green disabled:cursor-default disabled:opacity-40"
            >
              <Check size={13} /> Save &amp; apply
            </button>
          </div>
        </div>

        {info.openrouter && info.key_set && (
          <CreditsRow
            credits={credits}
            onRefresh={() =>
              fetchLLMCredits().then(setCredits).catch(() => setCredits(null))
            }
          />
        )}
      </div>
    </div>
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

/** One row in the provider list: active radio, label/endpoint, key state, and
 * a delete button (hidden for env-sourced read-only entries). */
function ProviderRow({
  p,
  active,
  busy,
  onActivate,
  onRemove,
}: {
  p: ProviderView;
  active: boolean;
  busy: boolean;
  onActivate: () => void;
  onRemove: () => void;
}) {
  return (
    <div className="flex items-center gap-3 border-b border-line/20 px-4 py-2.5 last:border-b-0">
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
          <span className="truncate">{p.label}</span>
          <span className="shrink-0 rounded bg-panel px-1 text-[10px] text-muted">
            {p.provider_name}
          </span>
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
  );
}

/** The inline form for adding a provider: label, adapter, endpoint, key. */
function AddProviderForm({
  busy,
  onCancel,
  onSubmit,
}: {
  busy: boolean;
  onCancel: () => void;
  onSubmit: (p: ProviderInput) => void;
}) {
  const [label, setLabel] = useState("");
  const [providerName, setProviderName] = useState("openai");
  const [endpoint, setEndpoint] = useState("");
  const [defaultModel, setDefaultModel] = useState("");
  const [key, setKey] = useState("");
  const canAdd = !busy && (label.trim() !== "" || endpoint.trim() !== "");

  const inputCls =
    "w-full rounded-md border border-line bg-panel px-2 py-1 font-mono text-[12px] text-bright outline-none placeholder:text-faint";
  return (
    <div className="flex flex-col gap-2">
      <input
        value={label}
        onChange={(e) => setLabel(e.target.value)}
        placeholder="Label (e.g. gm, My OpenRouter)"
        className={inputCls}
      />
      <select
        value={providerName}
        onChange={(e) => setProviderName(e.target.value)}
        className="w-full cursor-pointer rounded-md border border-line bg-panel px-2 py-1 text-[12px] text-bright outline-none"
      >
        {ADAPTERS.map((a) => (
          <option key={a.id} value={a.id}>
            {a.label}
          </option>
        ))}
      </select>
      <input
        value={endpoint}
        onChange={(e) => setEndpoint(e.target.value)}
        placeholder="Endpoint (blank = adapter default)"
        spellCheck={false}
        className={inputCls}
      />
      <input
        value={defaultModel}
        onChange={(e) => setDefaultModel(e.target.value)}
        placeholder="Default model (optional)"
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
              label: label.trim(),
              provider_name: providerName,
              endpoint: endpoint.trim(),
              default_model: defaultModel.trim(),
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

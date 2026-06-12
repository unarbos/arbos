import { useCallback, useEffect, useState } from "react";
import { Check, KeyRound, Pencil, Plus, Trash2, X } from "lucide-react";

import {
  deleteSecret,
  fetchSecrets,
  saveSecret,
  type SecretMeta,
} from "@/lib/api";

/**
 * The Settings tab's managed-secret panel: the user names a secret, labels it,
 * and gives it a value once. The value is write-only — the server never returns
 * it. By default the agent holds only the name: it can attach the secret to a
 * fetch request (allowlisted hosts, HTTPS) without the value ever entering its
 * context. Shell exposure is the explicit exception — an env-injected secret
 * is readable by the agent (`echo $NAME`) and can land in transcripts and
 * exports, so the toggle says exactly that instead of pretending otherwise.
 *
 * The section filters its own entries against the Settings search query; an
 * empty match still shows the header and the add affordance.
 */
export function SecretsSettings({ query }: { query: string }) {
  const [secrets, setSecrets] = useState<SecretMeta[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<Draft | null>(null);

  const reload = useCallback(async () => {
    try {
      setSecrets(await fetchSecrets());
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  const q = query.trim().toLowerCase();
  const visible = secrets.filter(
    (s) =>
      !q ||
      s.name.toLowerCase().includes(q) ||
      (s.label ?? "").toLowerCase().includes(q),
  );
  // A query that matches neither these entries nor the section's own keywords
  // hides the whole section, matching the other settings groups' behavior.
  if (q && visible.length === 0 && !"secrets environment".includes(q)) {
    return null;
  }

  const remove = async (name: string) => {
    try {
      await deleteSecret(name);
      await reload();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <div className="mb-6">
      <div className="mb-2 flex items-center justify-between px-1">
        <span className="text-[12px] text-muted select-none">Secrets</span>
        {!editing && (
          <button
            type="button"
            onClick={() => setEditing(blankDraft())}
            className="flex cursor-pointer items-center gap-1 rounded-md px-1.5 py-0.5 text-[11.5px] text-muted hover:bg-hover hover:text-bright"
          >
            <Plus size={12} /> Add secret
          </button>
        )}
      </div>

      <div className="overflow-hidden rounded-xl bg-card/50">
        <p className="border-b border-line/30 px-4 py-2.5 text-[11.5px] leading-relaxed text-faint">
          Encrypted on this machine and never shown again. The agent can use a
          secret by name on its HTTPS requests — allowed hosts only, the value
          never enters its context. Exposing one to shell tools is different:
          the agent can read those values.
        </p>

        {error && (
          <div className="border-b border-line/30 px-4 py-2 text-[11.5px] text-red">
            {error}
          </div>
        )}

        <div className="divide-y divide-line/30">
          {visible.map((s) =>
            // Match on the entry's *original* name, not the draft's editable
            // name — typing in the Name field must not unmount the form.
            editing && editing.original === s.name ? (
              <SecretForm
                key={s.name}
                draft={editing}
                existing
                onChange={setEditing}
                onCancel={() => setEditing(null)}
                onSaved={async () => {
                  setEditing(null);
                  await reload();
                }}
                onError={setError}
              />
            ) : (
              <SecretRow
                key={s.name}
                secret={s}
                onEdit={() => setEditing(fromMeta(s))}
                onDelete={() => void remove(s.name)}
              />
            ),
          )}

          {editing && editing.original === null && (
            <SecretForm
              key="__new__"
              draft={editing}
              existing={false}
              onChange={setEditing}
              onCancel={() => setEditing(null)}
              onSaved={async () => {
                setEditing(null);
                await reload();
              }}
              onError={setError}
            />
          )}

          {visible.length === 0 && !editing && (
            <div className="px-4 py-3 text-[12px] text-faint">
              No secrets yet.
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

/** One stored secret: label, name, host chips, and the env-injection badge. */
function SecretRow({
  secret,
  onEdit,
  onDelete,
}: {
  secret: SecretMeta;
  onEdit: () => void;
  onDelete: () => void;
}) {
  return (
    <div className="flex items-center justify-between gap-3 px-4 py-3">
      <div className="flex min-w-0 items-center gap-2.5">
        <KeyRound size={14} className="shrink-0 text-faint" />
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <span className="truncate font-mono text-[12.5px] text-bright">
              {secret.name}
            </span>
            {secret.env && (
              <span className="shrink-0 rounded bg-hover px-1.5 py-px text-[10px] text-muted">
                tool env
              </span>
            )}
          </div>
          <div className="mt-0.5 flex flex-wrap items-center gap-1.5 text-[11.5px] text-muted">
            {secret.label && <span className="truncate">{secret.label}</span>}
            {(secret.hosts ?? []).map((h) => (
              <span
                key={h}
                className="rounded bg-panel px-1.5 py-px font-mono text-[10.5px] text-faint"
              >
                {h}
              </span>
            ))}
          </div>
        </div>
      </div>
      <div className="flex shrink-0 items-center gap-1">
        <IconButton label="Edit" onClick={onEdit}>
          <Pencil size={13} />
        </IconButton>
        <IconButton label="Delete" onClick={onDelete}>
          <Trash2 size={13} />
        </IconButton>
      </div>
    </div>
  );
}

/** The add/edit form. Name is fixed once a secret exists; value is left blank
 * to keep the stored one. */
function SecretForm({
  draft,
  existing,
  onChange,
  onCancel,
  onSaved,
  onError,
}: {
  draft: Draft;
  existing: boolean;
  onChange: (d: Draft) => void;
  onCancel: () => void;
  onSaved: () => void | Promise<void>;
  onError: (msg: string) => void;
}) {
  const [busy, setBusy] = useState(false);

  const submit = async () => {
    setBusy(true);
    try {
      await saveSecret({
        name: draft.name.trim(),
        label: draft.label.trim(),
        value: draft.value,
        hosts: draft.hosts
          .split(",")
          .map((h) => h.trim())
          .filter(Boolean),
        env: draft.env,
      });
      await onSaved();
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const canSave =
    draft.name.trim() !== "" && (existing || draft.value !== "");

  return (
    <div className="flex flex-col gap-2.5 bg-panel/40 px-4 py-3.5">
      <div className="flex gap-2.5">
        <Field label="Name">
          <input
            value={draft.name}
            onChange={(e) =>
              onChange({ ...draft, name: e.target.value.toUpperCase() })
            }
            disabled={existing}
            placeholder="GITHUB_TOKEN"
            className="w-full rounded-md border border-line bg-panel px-2 py-1 font-mono text-[12px] text-bright outline-none placeholder:text-faint disabled:opacity-50"
          />
        </Field>
        <Field label="Label">
          <input
            value={draft.label}
            onChange={(e) => onChange({ ...draft, label: e.target.value })}
            placeholder="GitHub PAT for CI"
            className="w-full rounded-md border border-line bg-panel px-2 py-1 text-[12px] text-bright outline-none placeholder:text-faint"
          />
        </Field>
      </div>

      <Field label="Value">
        <input
          type="password"
          value={draft.value}
          onChange={(e) => onChange({ ...draft, value: e.target.value })}
          placeholder={existing ? "•••••••• (unchanged)" : "Paste secret value"}
          autoComplete="off"
          className="w-full rounded-md border border-line bg-panel px-2 py-1 font-mono text-[12px] text-bright outline-none placeholder:text-faint"
        />
      </Field>

      <Field label="Allowed hosts">
        <input
          value={draft.hosts}
          onChange={(e) => onChange({ ...draft, hosts: e.target.value })}
          placeholder="api.github.com, uploads.github.com"
          className="w-full rounded-md border border-line bg-panel px-2 py-1 font-mono text-[12px] text-bright outline-none placeholder:text-faint"
        />
      </Field>

      <label className="flex cursor-pointer items-start gap-2 text-[12px] text-muted">
        <input
          type="checkbox"
          checked={draft.env}
          onChange={(e) => onChange({ ...draft, env: e.target.checked })}
          className="mt-0.5 size-3.5 shrink-0 cursor-pointer accent-green"
        />
        <span>
          Available to the agent's shell tools as <code className="font-mono text-faint">${draft.name || "NAME"}</code>
          <span className="block text-faint">
            On: the agent can read the value (it sits in tool subprocess environments) and it may appear in transcripts and exports.
            Off: broker-only — attached to allowed hosts over HTTPS, never readable by the agent.
          </span>
        </span>
      </label>

      <div className="mt-0.5 flex items-center justify-end gap-2">
        <button
          type="button"
          onClick={onCancel}
          className="flex cursor-pointer items-center gap-1 rounded-md px-2 py-1 text-[12px] text-muted hover:bg-hover hover:text-bright"
        >
          <X size={13} /> Cancel
        </button>
        <button
          type="button"
          onClick={() => void submit()}
          disabled={!canSave || busy}
          className="flex cursor-pointer items-center gap-1 rounded-md bg-green/90 px-2.5 py-1 text-[12px] text-white hover:bg-green disabled:cursor-default disabled:opacity-40"
        >
          <Check size={13} /> Save
        </button>
      </div>
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="flex flex-1 flex-col gap-1">
      <span className="text-[11px] text-faint">{label}</span>
      {children}
    </label>
  );
}

function IconButton({
  label,
  onClick,
  children,
}: {
  label: string;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      className="flex size-7 cursor-pointer items-center justify-center rounded-md text-muted hover:bg-hover hover:text-bright"
    >
      {children}
    </button>
  );
}

/**
 * The form's editable shape. `original` is the name of the entry being edited
 * (which slot the form occupies), or null for a brand-new secret — kept
 * separate from the editable `name` so typing the name never moves the form.
 */
interface Draft {
  original: string | null;
  name: string;
  label: string;
  value: string;
  hosts: string;
  env: boolean;
}

function blankDraft(): Draft {
  // Broker-only by default: the agent can already use the secret over HTTPS
  // (fetch's auth) without ever reading it, so shell exposure — which makes
  // the value readable and transcript-visible — is the deliberate opt-in for
  // CLI tools that need $NAME, not the starting point.
  return { original: null, name: "", label: "", value: "", hosts: "", env: false };
}

function fromMeta(s: SecretMeta): Draft {
  return {
    original: s.name,
    name: s.name,
    label: s.label ?? "",
    value: "",
    hosts: (s.hosts ?? []).join(", "),
    env: s.env ?? false,
  };
}

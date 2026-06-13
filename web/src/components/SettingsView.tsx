import { useEffect, useState } from "react";
import {
  Cloud,
  KeyRound,
  Search,
  Settings,
  SlidersHorizontal,
  X,
} from "lucide-react";

import {
  fetchHostSettings,
  saveHostSettings,
  type HostSettings,
} from "@/lib/api";
import { ensureNotifyPermission } from "@/lib/notify";
import { setSetting, useSettings } from "@/lib/settings";
import { setTheme, useTheme } from "@/lib/theme";
import { THEMES } from "@/lib/themes";
import { ModelPicker } from "./ModelPicker";
import { ProviderSettings } from "./ProviderSettings";
import { SecretsSettings } from "./SecretsSettings";

interface Row {
  title: string;
  description: string;
  control: React.ReactNode;
}

interface Section {
  label: string;
  rows: Row[];
}

export type SettingsPage = "general" | "provider" | "secrets";

const PAGES: {
  id: SettingsPage;
  label: string;
  icon: React.ComponentType<{ size?: number; className?: string }>;
}[] = [
  { id: "general", label: "General", icon: Settings },
  { id: "provider", label: "Provider", icon: Cloud },
  { id: "secrets", label: "Secrets", icon: KeyRound },
];

/**
 * The settings panel as a tab — Cursor's settings layout on arbos's tokens:
 * a sidebar (identity, search, page nav) beside a scrolling column of
 * grouped setting cards, each row a title + description with its control
 * on the right. Provider and Secrets live on their own pages; a non-empty
 * search overrides the page choice and matches across all of them.
 * Settings persist in the localStorage-backed settings store (theme keeps
 * its own store; this panel just surfaces it). initialPage selects which
 * page opens first — the onboarding auto-open lands straight on Provider.
 */
export function SettingsView({ initialPage }: { initialPage?: SettingsPage }) {
  const settings = useSettings();
  const theme = useTheme();
  const [query, setQuery] = useState("");
  const [page, setPage] = useState<SettingsPage>(initialPage ?? "general");
  // The durable host preferences (the Go side's settings.json). null until
  // loaded — and stays null when the gateway has no settings store (e.g. no
  // config dir), which simply hides the Agent section.
  const [host, setHost] = useState<HostSettings | null>(null);

  useEffect(() => {
    fetchHostSettings()
      .then(setHost)
      .catch(() => setHost(null));
  }, []);

  const saveHost = (next: HostSettings) => {
    // Optimistic: the row reflects the pick immediately; a failed save
    // re-syncs from the file so the UI never shows a preference that
    // didn't land.
    setHost(next);
    saveHostSettings(next).catch(() => {
      fetchHostSettings()
        .then(setHost)
        .catch(() => setHost(null));
    });
  };

  const toggleNotifications = async (on: boolean) => {
    // The toggle only lands ON if the browser actually granted permission —
    // an enabled switch that can never notify would lie.
    if (on && !(await ensureNotifyPermission())) return;
    setSetting("systemNotifications", on);
  };

  const sections: Section[] = [
    {
      label: "Interface",
      rows: [
        {
          title: "Theme",
          description: "Color theme for the whole panel",
          control: (
            <select
              value={theme.id}
              onChange={(e) => setTheme(e.target.value)}
              className="cursor-pointer rounded-md border border-line bg-panel px-2 py-1 text-[12px] text-text outline-none"
            >
              {THEMES.map((t) => (
                <option key={t.id} value={t.id}>
                  {t.name}
                </option>
              ))}
            </select>
          ),
        },
      ],
    },
    {
      label: "Notifications",
      rows: [
        {
          title: "System Notifications",
          description:
            "Show a system notification when an agent finishes while the window is unfocused",
          control: (
            <Toggle
              on={settings.systemNotifications}
              onChange={(on) => void toggleNotifications(on)}
            />
          ),
        },
        {
          title: "Completion Sound",
          description: "Play a sound when an agent finishes responding",
          control: (
            <Toggle
              on={settings.completionSound}
              onChange={(on) => setSetting("completionSound", on)}
            />
          ),
        },
        {
          title: "Spoken Responses",
          description:
            "Read the agent's reply aloud when it finishes (replaces the completion sound)",
          control: (
            <Toggle
              on={settings.speakResponses}
              onChange={(on) => setSetting("speakResponses", on)}
            />
          ),
        },
      ],
    },
  ];

  if (host !== null) {
    sections.push({
      label: "Agent",
      rows: [
        {
          title: "Default Model",
          description:
            "Model new sessions start on (applies at the next arbos start) — unset follows the launch environment",
          control: (
            <div className="flex items-center gap-1">
              {host.default_model ? (
                <button
                  type="button"
                  aria-label="Use launch default"
                  title="Use launch default"
                  onClick={() => saveHost({ ...host, default_model: "" })}
                  className="flex size-6 cursor-pointer items-center justify-center rounded-md text-muted hover:bg-hover hover:text-bright"
                >
                  <X size={13} />
                </button>
              ) : null}
              <ModelPicker
                current={host.default_model ?? ""}
                onSelect={(id) => saveHost({ ...host, default_model: id })}
                side="down"
                align="right"
                emptyLabel="launch default"
              />
            </div>
          ),
        },
        {
          title: "Subagent Model",
          description:
            "Model delegated sub-agents run on — unset follows the main model",
          control: (
            <div className="flex items-center gap-1">
              {host.subagent_model ? (
                <button
                  type="button"
                  aria-label="Use main model"
                  title="Use main model"
                  onClick={() => saveHost({ ...host, subagent_model: "" })}
                  className="flex size-6 cursor-pointer items-center justify-center rounded-md text-muted hover:bg-hover hover:text-bright"
                >
                  <X size={13} />
                </button>
              ) : null}
              <ModelPicker
                current={host.subagent_model ?? ""}
                onSelect={(id) => saveHost({ ...host, subagent_model: id })}
                side="down"
                align="right"
                emptyLabel="main model"
              />
            </div>
          ),
        },
      ],
    });
  }

  const q = query.trim().toLowerCase();
  // A search spans every page — results would be invisible otherwise.
  const searching = q !== "";
  const showGeneral = searching || page === "general";
  const showProvider = searching || page === "provider";
  const showSecrets = searching || page === "secrets";

  const visible = sections
    .map((s) => ({
      ...s,
      rows: s.rows.filter(
        (r) =>
          !q ||
          r.title.toLowerCase().includes(q) ||
          r.description.toLowerCase().includes(q),
      ),
    }))
    .filter((s) => s.rows.length > 0);

  return (
    <div className="@container flex min-h-0 min-w-0 flex-1 bg-canvas">
      {/* In a narrow split pane the cards get all the room — the sidebar is
          navigation chrome, not content. */}
      <aside className="hidden w-56 shrink-0 flex-col gap-3 border-r border-line/50 px-3 py-4 @[32rem]:flex">
        <div className="flex items-center gap-2.5 px-1">
          <span className="flex size-7 shrink-0 items-center justify-center rounded-full bg-card text-[12px] font-medium text-bright">
            A
          </span>
          <span className="min-w-0">
            <span className="block truncate text-[12.5px] text-bright">Arbos</span>
            <span className="block truncate text-[11px] text-faint">
              Local workspace
            </span>
          </span>
        </div>

        <div className="flex items-center gap-2 rounded-md border border-line/60 px-2.5 py-1.5">
          <Search size={12} className="shrink-0 text-faint" />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search settings"
            className="min-w-0 flex-1 bg-transparent text-[12.5px] text-bright outline-none placeholder:text-faint"
          />
        </div>

        <nav className="flex flex-col gap-0.5">
          {PAGES.map(({ id, label, icon: Icon }) => (
            <button
              key={id}
              type="button"
              onClick={() => setPage(id)}
              className={`flex cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 text-left text-[12.5px] ${
                page === id && !searching
                  ? "bg-hover text-bright"
                  : "text-muted hover:bg-hover hover:text-bright"
              }`}
            >
              <Icon size={13} className="shrink-0 text-muted" />
              {label}
            </button>
          ))}
        </nav>
      </aside>

      <div className="min-h-0 min-w-0 flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-2xl px-4 py-5 @[32rem]:px-8 @[32rem]:py-6">
          {/* The sidebar disappears in a narrow pane; these pills are the
              page switcher there. */}
          <div className="mb-4 flex gap-1 @[32rem]:hidden">
            {PAGES.map(({ id, label }) => (
              <button
                key={id}
                type="button"
                onClick={() => setPage(id)}
                className={`cursor-pointer rounded-md px-2.5 py-1 text-[12px] ${
                  page === id && !searching
                    ? "bg-hover text-bright"
                    : "text-muted hover:bg-hover hover:text-bright"
                }`}
              >
                {label}
              </button>
            ))}
          </div>

          {showGeneral && visible.length === 0 && searching && (
            <div className="flex items-center gap-2 text-[12.5px] text-faint">
              <SlidersHorizontal size={13} />
              No settings match “{query.trim()}”
            </div>
          )}
          {showGeneral && visible.map((section) => (
            <div key={section.label} className="mb-6">
              <div className="mb-2 px-1 text-[12px] text-muted select-none">
                {section.label}
              </div>
              <div className="divide-y divide-line/30 overflow-hidden rounded-xl bg-card/50">
                {section.rows.map((row) => (
                  <div
                    key={row.title}
                    className="flex items-center justify-between gap-6 px-4 py-3"
                  >
                    <div className="min-w-0">
                      <div className="text-[13px] text-bright">{row.title}</div>
                      <div className="mt-0.5 text-[12px] text-muted">
                        {row.description}
                      </div>
                    </div>
                    <div className="shrink-0">{row.control}</div>
                  </div>
                ))}
              </div>
            </div>
          ))}
          {showProvider && <ProviderSettings query={query} />}
          {showSecrets && <SecretsSettings query={query} />}
        </div>
      </div>
    </div>
  );
}

/** Cursor's green pill switch. */
function Toggle({ on, onChange }: { on: boolean; onChange: (on: boolean) => void }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      onClick={() => onChange(!on)}
      className={`relative h-[18px] w-8 cursor-pointer rounded-full transition-colors ${
        on ? "bg-green" : "bg-faint/40"
      }`}
    >
      <span
        className={`absolute left-[2px] top-[2px] size-[14px] rounded-full bg-white shadow-sm transition-transform ${
          on ? "translate-x-[14px]" : ""
        }`}
      />
    </button>
  );
}

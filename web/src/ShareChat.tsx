import { useState } from "react";

import { ActivityView } from "./components/ActivityView";
import { ChatTab } from "./components/ChatTab";
import { HistoryView } from "./components/HistoryView";
import { PeopleSurface } from "./components/PeopleSurface";
import {
  ActivityButton,
  HistoryButton,
  PeopleButton,
  TabStrip,
  type TabInfo,
} from "./components/TabStrip";
import { ThemePicker } from "./components/ThemePicker";
import type { SharePerm } from "./lib/api";
import { peopleSurface } from "./lib/surface";
import { useMediaQuery } from "./lib/useMediaQuery";

/**
 * The share-mode view: a scoped session link drops the holder onto exactly one
 * chat, rendered by the REAL ChatTab inside the REAL tab-strip chrome — same
 * transcript, composer, themes, and History / Activity / People as the
 * workspace. History, Activity, and People are symmetrical: each is a top-row
 * button that opens (or focuses) its own singleton panel. It is the standard
 * endpoint minus the things a guest has no business touching: there is no
 * Settings, no "+" to open new tabs, and no "share agent" — and everything is
 * read-only when the grant is read.
 *
 * Layout: the pinned chat is always present, and a single companion panel
 * (History, Activity, or People) docks BESIDE it in a two-pane horizontal
 * split — so a guest can watch the agent work and talk to other participants
 * (or scan history/activity) at the same time, mirroring how the owner
 * workspace docks People as a sidebar. This is a fixed two-pane split rather
 * than the workspace's free split tree: a guest never accumulates panes, so
 * opening a second companion just replaces the one already docked. On phones
 * there is no room for two columns, so the split collapses to whichever panel
 * is active, full-bleed (the same rule the workspace uses below `sm`).
 *
 * There is no second chat client and no parallel feeds: the seam is scoped
 * server-side (the frame-filter locks it to this session and gates writes),
 * History is fed just this one chat, and Activity is filtered server-side to
 * this session — so the same components simply work under a narrower
 * permission. See ADR-0034.
 */

// The chat is pinned; one companion panel docks beside it. Keying companions
// by kind keeps opening idempotent: re-opening a companion that's already
// docked just focuses it, and opening a different one replaces it — a guest
// never accumulates panels.
type CompanionKind = "history" | "activity" | "people";
type PanelKind = "chat" | CompanionKind;
const CHAT_KEY = 1;
const KEY_OF: Record<PanelKind, number> = { chat: 1, history: 2, activity: 3, people: 4 };
const KIND_OF: Record<number, PanelKind> = { 1: "chat", 2: "history", 3: "activity", 4: "people" };
const PANEL_TITLE: Record<PanelKind, string> = {
  chat: "Shared conversation",
  history: "History",
  activity: "Agent activity",
  people: "People",
};

export function ShareChat({ session, perm }: { session: string; perm: SharePerm }) {
  const canWrite = perm === "write" || perm === "admin";

  // The chat is always open. At most one companion is docked beside it.
  // People opens docked by default the moment a guest joins (right after they
  // enter their name and this view mounts), so a shared link lands them able
  // to watch the agent AND talk to the other participants without a click.
  const [companion, setCompanion] = useState<CompanionKind | null>("people");
  // Which pane holds the keyboard / is shown full-bleed on a phone.
  const [active, setActive] = useState<PanelKind>("chat");
  const [chatTitle, setChatTitle] = useState(PANEL_TITLE.chat);
  const [chatBusy, setChatBusy] = useState(false);
  // The chat title is owned by its first prompt until the guest renames the
  // tab, which pins it the way the workspace pins a renamed tab.
  const [titlePinned, setTitlePinned] = useState(false);

  // Phones have no room for two columns: render only the active pane,
  // full-bleed. The companion is untouched — widen the window and it reappears
  // beside the chat. Mirrors App.tsx's narrow-collapse rule.
  const narrow = useMediaQuery("(max-width: 639px)");
  const split = companion !== null && !narrow;

  const openPanel = (kind: PanelKind) => {
    if (kind === "chat") {
      setActive("chat");
      return;
    }
    setCompanion(kind);
    setActive(kind);
  };

  const closeCompanion = () => {
    setCompanion(null);
    setActive("chat");
  };

  // On a phone the strip lists chat + the open companion as switchable tabs;
  // in split mode each pane gets its own one-tab strip, so the strip below is
  // only the phone/single-pane chrome.
  const singleStripTabs: TabInfo[] = [
    {
      key: CHAT_KEY,
      title: chatTitle,
      busy: chatBusy,
      kind: "chat",
      closable: false,
    },
    ...(companion
      ? [
          {
            key: KEY_OF[companion],
            title: PANEL_TITLE[companion],
            busy: false,
            kind: companion,
            closable: true,
          },
        ]
      : []),
  ];

  const noopDrag = {
    onStart: () => {},
    onDropTab: () => {},
    onEnd: () => {},
    onDropStrip: () => {},
  };

  const leadingButtons = (
    <>
      <HistoryButton onOpen={() => openPanel("history")} />
      <ActivityButton onOpen={() => openPanel("activity")} />
      <PeopleButton onOpen={() => openPanel("people")} />
    </>
  );

  const chatPane = (paneVisible: boolean) => (
    <ChatTab
      active={paneVisible}
      focused={paneVisible}
      focusTick={0}
      resumeId={session}
      readOnly={!canWrite}
      handle={{
        onBusy: setChatBusy,
        onTitle: (text) => {
          if (!titlePinned) setChatTitle(tabTitle(text));
        },
        // The chat's People rail opens the same panel as the top-row People
        // button. Only People is openable in this chrome — every other surface
        // (files, canvases) is 403 for a guest.
        onOpenSurface: (s) => {
          if (s.kind === "people") openPanel("people");
        },
      }}
    />
  );

  const companionPane = (kind: CompanionKind, paneVisible: boolean) =>
    kind === "history" ? (
      <HistoryView
        active={paneVisible}
        sessions={[{ id: session, title: chatTitle, updated_at: Date.now() }]}
        onOpenSession={() => setActive("chat")}
      />
    ) : kind === "activity" ? (
      <ActivityView readOnly onOpenChat={() => setActive("chat")} />
    ) : (
      <PeopleSurface
        surface={peopleSurface(session)}
        active={paneVisible}
        readOnly={!canWrite}
      />
    );

  // Two-pane split (desktop, a companion open): chat on the left, the
  // companion on the right, each under its own one-tab strip. Both panes stay
  // mounted and live so the agent transcript and the side chat update together.
  if (split && companion) {
    return (
      <div className="flex h-dvh flex-col bg-canvas text-text">
        <div className="flex min-h-0 flex-1">
          <div className="flex min-w-0 flex-1 flex-col border-r border-line">
            <div className="safe-pt shrink-0 bg-bar">
              <TabStrip
                tabs={[{ key: CHAT_KEY, title: chatTitle, busy: chatBusy, kind: "chat", closable: false }]}
                activeKey={CHAT_KEY}
                canClose={false}
                onActivate={() => setActive("chat")}
                onClose={() => {}}
                onRename={(_key, title) => {
                  setTitlePinned(true);
                  setChatTitle(title);
                }}
                onShare={() => {}}
                drag={noopDrag}
                leading={leadingButtons}
              />
            </div>
            <div className="relative min-h-0 flex-1">
              <div className="absolute inset-0 flex min-h-0 flex-col">{chatPane(true)}</div>
            </div>
          </div>
          <div className="flex min-w-0 flex-1 flex-col">
            <div className="safe-pt shrink-0 bg-bar">
              <TabStrip
                tabs={[
                  {
                    key: KEY_OF[companion],
                    title: PANEL_TITLE[companion],
                    busy: false,
                    kind: companion,
                    closable: true,
                  },
                ]}
                activeKey={KEY_OF[companion]}
                canClose
                onActivate={() => setActive(companion)}
                onClose={closeCompanion}
                onRename={() => {}}
                onShare={() => {}}
                drag={noopDrag}
                actions={<ThemePicker />}
              />
            </div>
            <div className="relative min-h-0 flex-1">
              <div className="absolute inset-0 flex min-h-0 flex-col">
                {companionPane(companion, true)}
              </div>
            </div>
          </div>
        </div>
      </div>
    );
  }

  // Single pane (phone, or no companion open): chat + companion as switchable
  // tabs in one strip, only the active one visible.
  return (
    <div className="flex h-dvh flex-col bg-canvas text-text">
      <div className="safe-pt shrink-0 bg-bar">
        <TabStrip
          tabs={singleStripTabs}
          activeKey={KEY_OF[active]}
          canClose
          onActivate={(key) => setActive(KIND_OF[key] ?? "chat")}
          onClose={(key) => {
            if (KIND_OF[key] !== "chat") closeCompanion();
          }}
          onRename={(key, title) => {
            if (KIND_OF[key] === "chat") {
              setTitlePinned(true);
              setChatTitle(title);
            }
          }}
          onShare={() => {}}
          drag={noopDrag}
          leading={leadingButtons}
          actions={<ThemePicker />}
        />
      </div>

      <div className="relative min-h-0 flex-1">
        <div className={active === "chat" ? "absolute inset-0 flex min-h-0 flex-col" : "hidden"}>
          {chatPane(active === "chat")}
        </div>
        {companion && (
          <div
            className={active === companion ? "absolute inset-0 flex min-h-0 flex-col" : "hidden"}
          >
            {companionPane(companion, active === companion)}
          </div>
        )}
      </div>
    </div>
  );
}

const TAB_TITLE_MAX = 40;

/** First line of the first prompt, truncated — mirrors App's tab titles. */
function tabTitle(text: string): string {
  const line = text.split("\n")[0].trim();
  return line.length > TAB_TITLE_MAX ? line.slice(0, TAB_TITLE_MAX) + "…" : line;
}

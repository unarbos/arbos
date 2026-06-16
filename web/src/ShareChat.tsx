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

/**
 * The share-mode view: a scoped session link drops the holder onto exactly one
 * chat, rendered by the REAL ChatTab inside the REAL tab-strip chrome — same
 * transcript, composer, themes, and History / Activity / People as the
 * workspace. History, Activity, and People are symmetrical: each is a top-row
 * button that opens (or focuses) its own singleton panel. It is the standard
 * endpoint minus the things a guest has no business touching: there is no
 * Settings, no "+" to open new tabs, no split, and no "share agent" — and
 * everything is read-only when the grant is read.
 *
 * There is no second chat client and no parallel feeds: the seam is scoped
 * server-side (the frame-filter locks it to this session and gates writes),
 * History is fed just this one chat, and Activity is filtered server-side to
 * this session — so the same components simply work under a narrower
 * permission. See ADR-0034.
 */

// One pane, a fixed set of panels keyed by kind — each is a singleton, so a
// guest never accumulates tabs. The chat is pinned (always open, never
// closable); History, Activity, and People open on demand. Keying by kind
// (rather than a running counter) makes opening idempotent: a second open just
// focuses the panel that's already there.
type PanelKind = "chat" | "history" | "activity" | "people";
const TAB_KEY: Record<PanelKind, number> = { chat: 1, history: 2, activity: 3, people: 4 };
const KIND_OF: Record<number, PanelKind> = { 1: "chat", 2: "history", 3: "activity", 4: "people" };
const PANEL_TITLE: Record<PanelKind, string> = {
  chat: "Shared conversation",
  history: "History",
  activity: "Agent activity",
  people: "People",
};

export function ShareChat({ session, perm }: { session: string; perm: SharePerm }) {
  const canWrite = perm === "write" || perm === "admin";

  // Which panels are open, in tab order (chat is always first), and which is
  // visible.
  const [open, setOpen] = useState<PanelKind[]>(["chat"]);
  const [active, setActive] = useState<PanelKind>("chat");
  const [chatTitle, setChatTitle] = useState(PANEL_TITLE.chat);
  const [chatBusy, setChatBusy] = useState(false);
  // The chat title is owned by its first prompt until the guest renames the
  // tab, which pins it the way the workspace pins a renamed tab.
  const [titlePinned, setTitlePinned] = useState(false);

  const openPanel = (kind: PanelKind) => {
    setOpen((prev) => (prev.includes(kind) ? prev : [...prev, kind]));
    setActive(kind);
  };

  const closeTab = (key: number) => {
    const kind = KIND_OF[key];
    if (!kind || kind === "chat") return; // the chat can't be closed
    setOpen((prev) => prev.filter((k) => k !== kind));
    if (active === kind) setActive("chat");
  };

  const titleFor = (kind: PanelKind) => (kind === "chat" ? chatTitle : PANEL_TITLE[kind]);

  const stripTabs: TabInfo[] = open.map((kind) => ({
    key: TAB_KEY[kind],
    title: titleFor(kind),
    busy: kind === "chat" ? chatBusy : false,
    kind,
    closable: kind !== "chat",
  }));

  const focusChat = () => setActive("chat");

  return (
    <div className="flex h-dvh flex-col bg-canvas text-text">
      <div className="safe-pt shrink-0 bg-bar">
        <TabStrip
          tabs={stripTabs}
          activeKey={TAB_KEY[active]}
          canClose
          onActivate={(key) => setActive(KIND_OF[key] ?? "chat")}
          onClose={closeTab}
          onRename={(key, title) => {
            if (KIND_OF[key] === "chat") {
              setTitlePinned(true);
              setChatTitle(title);
            }
          }}
          onShare={() => {}}
          drag={{ onStart: () => {}, onDropTab: () => {}, onEnd: () => {}, onDropStrip: () => {} }}
          leading={
            <>
              <HistoryButton onOpen={() => openPanel("history")} />
              <ActivityButton onOpen={() => openPanel("activity")} />
              <PeopleButton onOpen={() => openPanel("people")} />
            </>
          }
          actions={<ThemePicker />}
        />
      </div>

      <div className="relative min-h-0 flex-1">
        {open.map((kind) => {
          const visible = kind === active;
          return (
            <div
              key={kind}
              className={visible ? "absolute inset-0 flex min-h-0 flex-col" : "hidden"}
            >
              {kind === "chat" ? (
                <ChatTab
                  active={visible}
                  focused={visible}
                  focusTick={0}
                  resumeId={session}
                  readOnly={!canWrite}
                  handle={{
                    onBusy: setChatBusy,
                    onTitle: (text) => {
                      if (!titlePinned) setChatTitle(tabTitle(text));
                    },
                    // The chat's People rail opens the same panel as the
                    // top-row People button. Only People is openable in this
                    // chrome — every other surface (files, canvases) is 403
                    // for a guest.
                    onOpenSurface: (s) => {
                      if (s.kind === "people") openPanel("people");
                    },
                  }}
                />
              ) : kind === "history" ? (
                <HistoryView
                  active={visible}
                  sessions={[{ id: session, title: chatTitle, updated_at: Date.now() }]}
                  onOpenSession={focusChat}
                />
              ) : kind === "activity" ? (
                <ActivityView readOnly onOpenChat={focusChat} />
              ) : (
                <PeopleSurface
                  surface={peopleSurface(session)}
                  active={visible}
                  readOnly={!canWrite}
                />
              )}
            </div>
          );
        })}
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

import { useCallback, useEffect, useRef, useState } from "react";
import {
  Bot,
  ChevronDown,
  Loader2,
  MoreHorizontal,
  Send,
} from "lucide-react";

import {
  addMessengerBot,
  connectMessenger,
  removeMessengerBot,
  setMessengerBotTools,
  type MessengerBot,
  type MessengerConvo,
} from "@/lib/messenger";
import { errMsg, toastError } from "@/lib/toast";

/**
 * A messenger tab: one Telegram connector over a real agent chat. Fresh tabs
 * ask for a bot token (or reattach to a connected bot); once Telegram is
 * linked, the body IS the normal ChatTab bound to the bridged session —
 * attachments, dictation, canvases, tool calls, sub-agents, everything — and
 * the bridge mirrors the same session to the phone. The chrome this file
 * owns is just the connector strip on top.
 */
export function MessengerView({
  botId,
  onBot,
  renderChat,
}: {
  /** The connected bot this tab is bound to; undefined = setup. */
  botId?: number;
  /** Bind (or unbind) the tab to a bot; title follows the bot's name. */
  onBot: (id: number | undefined, title?: string) => void;
  /** Render the fully wired agent chat for a bridged session (App owns the
   *  ChatTab plumbing — surfaces, runs, terminals all route like any chat). */
  renderChat: (sessionId: string) => React.ReactNode;
}) {
  const [bots, setBots] = useState<MessengerBot[] | null>(null);
  const [convos, setConvos] = useState<MessengerConvo[]>([]);
  const [selected, setSelected] = useState<string | null>(null);

  useEffect(() => {
    const stop = connectMessenger((ev) => {
      switch (ev.type) {
        case "state":
          setBots(ev.bots ?? []);
          setConvos(ev.conversations ?? []);
          break;
        case "conversation":
          setConvos((cs) => {
            const next = cs.filter((c) => c.id !== ev.conversation.id);
            next.push(ev.conversation);
            return next;
          });
          break;
        default: {
          const exhaustive: never = ev;
          return exhaustive;
        }
      }
    });
    return stop;
  }, []);

  const bot = botId !== undefined ? (bots ?? []).find((b) => b.id === botId) : undefined;

  // This tab's conversations: the bound bot's only, newest first. The
  // selection sticks while its conversation exists; otherwise the most
  // recently active one wins (a brand-new link lands here automatically).
  const sorted = convos
    .filter((c) => c.bot_id === botId)
    .sort((a, b) => (b.last_at ?? 0) - (a.last_at ?? 0));
  const current = sorted.find((c) => c.id === selected) ?? sorted[0] ?? null;

  const disconnect = useCallback(() => {
    if (!bot) return;
    if (!window.confirm(`Disconnect @${bot.username}? The conversation stays in history.`)) {
      return;
    }
    removeMessengerBot(bot.id)
      .then(() => onBot(undefined))
      .catch((e: unknown) => toastError(`Disconnect failed: ${errMsg(e)}`));
  }, [bot, onBot]);

  const setTools = useCallback(
    (tools: boolean) => {
      if (!bot) return;
      setMessengerBotTools(bot.id, tools).catch((e: unknown) =>
        toastError(`Permissions change failed: ${errMsg(e)}`),
      );
    },
    [bot],
  );

  if (bots === null) {
    return <div className="flex flex-1 items-center justify-center text-faint">Loading…</div>;
  }

  // No binding (fresh tab), or the bound bot is gone (disconnected from
  // another tab): set up a connector.
  if (botId === undefined || !bot) {
    return <Setup existing={bots} onPick={(b) => onBot(b.id, `@${b.username}`)} />;
  }

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      {/* The connector strip: which Telegram thread this chat is wired to. */}
      <div className="flex h-8 shrink-0 items-center gap-2 border-b border-line/60 px-4">
        <Send size={12} className="shrink-0 text-accent" />
        {sorted.length > 1 ? (
          <ConvoPicker convos={sorted} current={current} onPick={(id) => setSelected(id)} />
        ) : (
          <span className="min-w-0 truncate text-[12.5px] font-medium text-bright">
            {current?.title ?? `@${bot.username}`}
          </span>
        )}
        <span className="shrink-0 text-[11px] text-faint">via @{bot.username}</span>
        {!bot.tools && (
          <span
            title="Tools are off for this connector — chat only"
            className="shrink-0 rounded bg-panel px-1.5 py-px text-[10px] text-muted"
          >
            tools off
          </span>
        )}
        {current?.busy && (
          <span className="flex shrink-0 items-center gap-1 text-[11px] text-muted">
            <Loader2 size={10} className="animate-spin text-accent" />
            replying on Telegram
          </span>
        )}
        <span className="flex-1" />
        <Menu bot={bot} onSetTools={setTools} onDisconnect={disconnect} />
      </div>

      {current ? (
        renderChat(current.session_id)
      ) : (
        <LinkHint bot={bot} />
      )}
    </div>
  );
}

/** Token entry, or one tap to reattach a bot that's already connected. */
function Setup({
  existing,
  onPick,
}: {
  existing: MessengerBot[];
  onPick: (bot: MessengerBot) => void;
}) {
  const [token, setToken] = useState("");
  const [tools, setTools] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = () => {
    const t = token.trim();
    if (!t || busy) return;
    setBusy(true);
    setError(null);
    addMessengerBot(t, tools)
      .then(onPick)
      .catch((e: unknown) => {
        setError(errMsg(e));
        setBusy(false);
      });
  };

  return (
    <div className="flex min-h-0 min-w-0 flex-1 items-center justify-center overflow-y-auto">
      <div className="mx-4 w-full max-w-md">
        <div className="mb-1 flex items-center gap-2 text-[14px] font-medium text-bright">
          <Send size={15} className="text-accent" />
          Talk to arbos on Telegram
        </div>
        <p className="mb-4 text-[12.5px] leading-relaxed text-muted">
          Message{" "}
          <a
            href="https://t.me/BotFather"
            target="_blank"
            rel="noreferrer"
            className="text-accent hover:underline"
          >
            @BotFather
          </a>
          , send <code className="rounded bg-card px-1 font-mono text-[11.5px]">/newbot</code>,
          and paste the token here. This tab becomes the conversation — a full
          agent chat, synced both ways with your phone.
        </p>
        <input
          value={token}
          onChange={(e) => setToken(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") submit();
          }}
          placeholder="Bot token from @BotFather"
          autoFocus
          className="mb-2 w-full rounded-md border border-line bg-panel px-3 py-2 font-mono text-[12px] text-bright outline-none placeholder:font-sans placeholder:text-faint focus:border-accent/50"
        />
        <ToolsToggle value={tools} onChange={setTools} className="mb-3" />
        {error && <div className="mb-3 text-[12px] leading-snug text-red">{error}</div>}
        <div className="flex justify-end">
          <button
            type="button"
            onClick={submit}
            disabled={!token.trim() || busy}
            className="flex cursor-pointer items-center gap-1.5 rounded-md bg-btn px-3.5 py-1.5 text-[12.5px] font-medium text-canvas transition-opacity hover:opacity-90 disabled:cursor-default disabled:opacity-40"
          >
            {busy && <Loader2 size={11} className="animate-spin" />}
            Connect
          </button>
        </div>

        {existing.length > 0 && (
          <div className="mt-6 border-t border-line/60 pt-4">
            <div className="mb-2 text-[11px] uppercase tracking-wider text-faint select-none">
              Already connected
            </div>
            <div className="flex flex-wrap gap-1.5">
              {existing.map((b) => (
                <button
                  key={b.id}
                  type="button"
                  onClick={() => onPick(b)}
                  className="flex cursor-pointer items-center gap-1.5 rounded-md border border-line/70 bg-panel px-2.5 py-1 text-[12px] text-text transition-colors hover:bg-hover hover:text-bright"
                >
                  <Bot size={11} className="text-muted" />
                  @{b.username}
                  {!b.tools && <span className="text-faint">tools off</span>}
                </button>
              ))}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

/** The connector's one permission switch: full chat powers, or chat only. */
function ToolsToggle({
  value,
  onChange,
  className = "",
}: {
  value: boolean;
  onChange: (v: boolean) => void;
  className?: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={value}
      onClick={() => onChange(!value)}
      className={`flex w-full cursor-pointer items-center gap-2.5 rounded-md border border-line/70 bg-panel px-3 py-2 text-left transition-colors hover:bg-hover/40 ${className}`}
    >
      <span
        className={`relative h-3.5 w-6 shrink-0 rounded-full transition-colors ${
          value ? "bg-accent" : "bg-line"
        }`}
      >
        <span
          className={`absolute top-0.5 size-2.5 rounded-full bg-canvas transition-[left] ${
            value ? "left-3" : "left-0.5"
          }`}
        />
      </span>
      <span className="min-w-0">
        <span className="block text-[12px] text-bright">Full permissions</span>
        <span className="block text-[11.5px] leading-snug text-muted">
          {value
            ? "Edit files, run commands — everything a normal chat can do."
            : "Chat only — no file access, shell, or web."}
        </span>
      </span>
    </button>
  );
}

/** The bot is connected but Telegram hasn't spoken yet — link the two. */
function LinkHint({ bot }: { bot: MessengerBot }) {
  return (
    <div className="flex min-h-0 flex-1 items-center justify-center">
      <div className="max-w-sm px-4 text-center">
        <Bot size={20} className="mx-auto mb-3 text-faint" />
        <div className="mb-1 text-[13px] text-bright">Almost there</div>
        <p className="mb-4 text-[12.5px] leading-relaxed text-muted">
          Send @{bot.username} one message on Telegram to link this chat to
          your account — then talk from either side.
        </p>
        <a
          href={`https://t.me/${bot.username}`}
          target="_blank"
          rel="noreferrer"
          className="inline-flex items-center gap-1.5 rounded-md bg-btn px-3.5 py-1.5 text-[12.5px] font-medium text-canvas transition-opacity hover:opacity-90"
        >
          <Send size={11} />
          Open @{bot.username}
        </a>
      </div>
    </div>
  );
}

/** Switch between this bot's conversations (groups, other chats). */
function ConvoPicker({
  convos,
  current,
  onPick,
}: {
  convos: MessengerConvo[];
  current: MessengerConvo | null;
  onPick: (id: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [open]);

  return (
    <div ref={rootRef} className="relative min-w-0">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex min-w-0 cursor-pointer items-center gap-1 rounded-md px-1.5 py-0.5 text-[12.5px] font-medium text-bright transition-colors hover:bg-hover"
      >
        <span className="min-w-0 truncate">{current?.title}</span>
        <ChevronDown size={11} className="shrink-0 text-muted" />
      </button>
      {open && (
        <div className="absolute left-0 top-full z-50 mt-1 flex w-56 flex-col rounded-lg border border-line bg-card py-1 shadow-xl shadow-black/40">
          {convos.map((c) => (
            <button
              key={c.id}
              type="button"
              onClick={() => {
                setOpen(false);
                onPick(c.id);
              }}
              className={`flex w-full cursor-pointer items-center gap-2 px-3 py-1.5 text-left text-[12.5px] transition-colors hover:bg-hover ${
                c.id === current?.id ? "text-bright" : "text-text"
              }`}
            >
              <span className="min-w-0 flex-1 truncate">{c.title}</span>
              {c.busy && <Loader2 size={10} className="shrink-0 animate-spin text-accent" />}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/** The rarely-used actions, out of the way: the permission switch and
 *  disconnect. */
function Menu({
  bot,
  onSetTools,
  onDisconnect,
}: {
  bot: MessengerBot;
  onSetTools: (tools: boolean) => void;
  onDisconnect: () => void;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [open]);

  return (
    <div ref={rootRef} className="relative">
      <button
        type="button"
        title="Connector options"
        onClick={() => setOpen((v) => !v)}
        className="flex size-6 cursor-pointer items-center justify-center rounded-md text-muted transition-colors hover:bg-hover hover:text-text"
      >
        <MoreHorizontal size={13} />
      </button>
      {open && (
        <div className="absolute right-0 top-full z-50 mt-1 flex w-72 flex-col gap-1 rounded-lg border border-line bg-card p-1.5 shadow-xl shadow-black/40">
          <ToolsToggle value={bot.tools} onChange={onSetTools} />
          <button
            type="button"
            onClick={() => {
              setOpen(false);
              onDisconnect();
            }}
            className="flex w-full cursor-pointer items-center gap-2 rounded-md px-3 py-1.5 text-left text-[12.5px] text-text transition-colors hover:bg-hover hover:text-red"
          >
            Disconnect this bot
          </button>
        </div>
      )}
    </div>
  );
}

/**
 * The messenger bridge's client surface: bot registry CRUD over REST and a
 * live registry stream over its own WebSocket. Conversation content never
 * crosses this seam — a bridged conversation is a normal kernel session, and
 * the panel renders it with the real ChatTab over the control seam. Types
 * mirror internal/messenger.
 */

/** One registered bot — the token never crosses back out of the gateway. */
export interface MessengerBot {
  id: number;
  username: string;
  /** The connector's one permission switch: tools on = a normal chat. */
  tools: boolean;
  /** Every bot locks to its first sender; this is who. */
  bound_name?: string;
  added_at: number; // unix milliseconds
}

/** One Telegram chat bridged to one kernel session. */
export interface MessengerConvo {
  id: string;
  bot_id: number;
  chat_id: number;
  title: string;
  kind: string; // private | group | supergroup
  session_id: string;
  last_at?: number; // unix milliseconds
  /** A bridge-run turn is in flight (phone-initiated). */
  busy?: boolean;
}

export type MessengerEvent =
  | { type: "state"; bots: MessengerBot[]; conversations: MessengerConvo[] }
  | { type: "conversation"; conversation: MessengerConvo };

export async function fetchMessengerState(): Promise<{
  bots: MessengerBot[];
  conversations: MessengerConvo[];
}> {
  const res = await fetch("/api/messenger/state");
  if (!res.ok) throw new Error(await errText(res, `messenger: ${res.status}`));
  const body = (await res.json()) as {
    bots?: MessengerBot[];
    conversations?: MessengerConvo[];
  };
  return { bots: body.bots ?? [], conversations: body.conversations ?? [] };
}

/** Register a bot token; the gateway validates it against Telegram first. */
export async function addMessengerBot(
  token: string,
  tools: boolean,
): Promise<MessengerBot> {
  const res = await fetch("/api/messenger/bots", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ token, tools }),
  });
  if (!res.ok) throw new Error(await errText(res, `add bot: ${res.status}`));
  return (await res.json()) as MessengerBot;
}

/** Flip a connector's permission switch; applies from the next turn. */
export async function setMessengerBotTools(
  id: number,
  tools: boolean,
): Promise<MessengerBot> {
  const res = await fetch(`/api/messenger/bots/${id}`, {
    method: "PATCH",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ tools }),
  });
  if (!res.ok) throw new Error(await errText(res, `set tools: ${res.status}`));
  return (await res.json()) as MessengerBot;
}

/** Stop and forget a bot (its sessions stay in history). */
export async function removeMessengerBot(id: number): Promise<void> {
  const res = await fetch(`/api/messenger/bots/${id}`, { method: "DELETE" });
  if (!res.ok) throw new Error(await errText(res, `remove bot: ${res.status}`));
}

/** How long a dropped stream waits before redialing. */
const RECONNECT_MS = 3000;

/**
 * Subscribe to the bridge's registry stream. The first frame is always a full
 * state snapshot, and the socket redials itself until the returned cleanup
 * runs — a fresh snapshot arrives on every reconnect, so the panel can treat
 * "state" as authoritative and the increments as cheap.
 */
export function connectMessenger(onEvent: (ev: MessengerEvent) => void): () => void {
  let ws: WebSocket | null = null;
  let timer: number | null = null;
  let closed = false;

  const dial = () => {
    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    ws = new WebSocket(`${proto}//${location.host}/api/messenger/ws`);
    ws.onmessage = (e) => {
      try {
        onEvent(JSON.parse(e.data as string) as MessengerEvent);
      } catch {
        // a malformed frame is the server's bug; don't kill the stream over it
      }
    };
    ws.onclose = () => {
      if (closed) return;
      timer = window.setTimeout(dial, RECONNECT_MS);
    };
  };
  dial();

  return () => {
    closed = true;
    if (timer !== null) window.clearTimeout(timer);
    ws?.close();
  };
}

async function errText(res: Response, fallback: string): Promise<string> {
  return (await res.text().catch(() => "")).trim() || fallback;
}

/**
 * MatrixClient — the browser end of a session's Matrix room (ADR-0041 P2).
 *
 * A seated guest's `/api/me` hands their browser its own scoped Matrix access
 * token (see MatrixSession); this drives the Client-Server API directly as that
 * identity — a thin translator, like SeamClient, never re-implementing turn
 * logic. It surfaces LIVE room events only: history comes from the replay
 * endpoint, so the initial sync is used only to capture the stream position and
 * the loop emits what arrives after. Membership is the authority — the token
 * works only while the guest is a joined member, so a kick (revoke) ends access.
 */

import type { MatrixSession } from "./api";

export type MatrixState = "idle" | "syncing" | "closed";

/** One decoded room-timeline event the client surfaces to the UI. body is the
 *  human-readable text (an m.room.message, a chat note, a tool result); the
 *  arbos metadata under the life.arbos content extension is lifted out when
 *  present so an arbos client can attribute and classify it. */
export interface RoomEvent {
  eventId: string;
  type: string; // "m.room.message" | "life.arbos.chat_note" | "life.arbos.tool_result" | …
  sender: string; // full Matrix user id
  ts: number; // origin_server_ts (unix ms)
  body: string;
  kind?: string; // arbos EventKind: "user_message" | "assistant_message" | …
  role?: string; // "user" | "assistant"
  author?: string; // display name of a multi-party participant, when set
}

export interface MatrixHandlers {
  onState?: (s: MatrixState) => void;
  /** New live room events since the last sync (never the backlog). */
  onEvents?: (events: RoomEvent[]) => void;
  onError?: (msg: string) => void;
}

/** The content key the arbos mirror stamps its structured metadata under, so a
 *  generic client renders the prose body and an arbos client reads the rest. */
const ARBOS_EXT = "life.arbos";
const SYNC_TIMEOUT_MS = 30000;
const SYNC_RETRY_MS = 2000;

interface SyncTimelineEvent {
  event_id: string;
  type: string;
  sender: string;
  origin_server_ts: number;
  content?: Record<string, unknown>;
}

interface SyncResponse {
  next_batch: string;
  rooms?: {
    join?: Record<string, { timeline?: { events?: SyncTimelineEvent[] } }>;
  };
}

export class MatrixClient {
  private readonly session: MatrixSession;
  private readonly handlers: MatrixHandlers;
  private since = "";
  private closed = false;
  private abort: AbortController | null = null;

  constructor(session: MatrixSession, handlers: MatrixHandlers) {
    this.session = session;
    this.handlers = handlers;
  }

  /** Begin syncing: one initial sync to capture the stream position (its
   *  backlog is intentionally dropped — the replay endpoint owns history), then
   *  a long-poll loop that emits live events. */
  start(): void {
    this.handlers.onState?.("syncing");
    void (async () => {
      try {
        const first = await this.sync(true);
        this.since = first.next_batch;
      } catch (e) {
        if (!this.closed) this.handlers.onError?.(errMsg(e));
      }
      await this.loop();
    })();
  }

  stop(): void {
    this.closed = true;
    this.abort?.abort();
  }

  /** Base origin for the Client-Server API. Empty homeserver means same origin:
   *  the gateway reverse-proxies /_matrix onto its own door (withMatrixProxy),
   *  so the homeserver is reachable wherever the SPA loaded from — including a
   *  remote browser over the forest tunnel (ADR-0041 H1). */
  private base(): string {
    return this.session.homeserver || window.location.origin;
  }

  private async loop(): Promise<void> {
    while (!this.closed) {
      try {
        const resp = await this.sync(false);
        this.since = resp.next_batch;
        const events = this.decode(resp);
        if (events.length > 0) this.handlers.onEvents?.(events);
      } catch (e) {
        if (this.closed) break;
        this.handlers.onError?.(errMsg(e));
        await sleep(SYNC_RETRY_MS); // a transient sync failure: back off and retry
      }
    }
    this.handlers.onState?.("closed");
  }

  private async sync(initial: boolean): Promise<SyncResponse> {
    const url = new URL(this.base() + "/_matrix/client/v3/sync");
    // The initial sync returns immediately (we only want next_batch); the loop
    // long-polls so live events arrive promptly without busy-waiting.
    url.searchParams.set("timeout", initial ? "0" : String(SYNC_TIMEOUT_MS));
    if (this.since) url.searchParams.set("since", this.since);
    this.abort = new AbortController();
    const res = await fetch(url.toString(), {
      headers: { Authorization: `Bearer ${this.session.access_token}` },
      signal: this.abort.signal,
    });
    if (!res.ok) throw new Error(`sync: ${res.status}`);
    return (await res.json()) as SyncResponse;
  }

  private decode(resp: SyncResponse): RoomEvent[] {
    // This client is scoped to a single room (the guest path). The operator's
    // multi-room client is a later increment; without a room there's nothing
    // to decode here.
    if (!this.session.room_id) return [];
    const events = resp.rooms?.join?.[this.session.room_id]?.timeline?.events ?? [];
    const out: RoomEvent[] = [];
    for (const ev of events) {
      const content = ev.content ?? {};
      const ext = content[ARBOS_EXT] as Record<string, unknown> | undefined;
      const body = typeof content.body === "string" ? content.body : "";
      if (!body) continue; // a structured-only event with no prose we render yet
      const author = ext?.author ?? content.author;
      out.push({
        eventId: ev.event_id,
        type: ev.type,
        sender: ev.sender,
        ts: ev.origin_server_ts,
        body,
        kind: typeof ext?.kind === "string" ? ext.kind : undefined,
        role: typeof ext?.role === "string" ? ext.role : undefined,
        author: typeof author === "string" ? author : undefined,
      });
    }
    return out;
  }

  /**
   * Send a plain room message as this guest's own identity. It is a normal
   * m.room.message (no arbos metadata), so the agent's room watcher sees it as
   * an external message and drives a turn; the reply mirrors back to the room,
   * where this client receives it on the next sync.
   */
  async send(body: string): Promise<void> {
    if (!this.session.room_id) throw new Error("no room to send to");
    const txn = `arbos-${Date.now()}-${Math.random().toString(36).slice(2)}`;
    const url =
      this.base() +
      `/_matrix/client/v3/rooms/${encodeURIComponent(this.session.room_id)}` +
      `/send/m.room.message/${encodeURIComponent(txn)}`;
    const res = await fetch(url, {
      method: "PUT",
      headers: {
        Authorization: `Bearer ${this.session.access_token}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ msgtype: "m.text", body }),
    });
    if (!res.ok) throw new Error(`send: ${res.status}`);
  }
}

function errMsg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

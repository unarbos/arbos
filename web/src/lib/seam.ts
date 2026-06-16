/**
 * SeamClient — the browser end of the arbos control seam.
 *
 * One WebSocket = one control.Serve connection on the gateway: each text
 * message out is one client frame, each message in is one server frame
 * (see internal/control/server.go). A thin translator, like every other
 * arbos frontend — it never re-implements turn logic.
 */

import type {
  ClientFrame,
  ContentBlock,
  Envelope,
  Intent,
  QuestionAnswer,
  ServerFrame,
} from "./types";

export type ConnectionState = "idle" | "connecting" | "open" | "closed";

export interface SeamHandlers {
  onState?: (s: ConnectionState) => void;
  onSession?: (id: string) => void;
  /** A fork this connection requested succeeded and is now bound. */
  onForked?: (id: string) => void;
  onEnvelope?: (env: Envelope) => void;
  onError?: (msg: string) => void;
  /** Outbox delivery (gateway broadcast): an ambient message between turns. */
  /** Outbox delivery; session is the owning chat ("" = ambient broadcast). */
  onNotice?: (text: string, session: string) => void;
}

function wsUrl(): string {
  const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
  return `${proto}//${window.location.host}/api/ws`;
}

export class SeamClient {
  private ws: WebSocket | null = null;
  private handlers: SeamHandlers;
  private _state: ConnectionState = "idle";

  constructor(handlers: SeamHandlers) {
    this.handlers = handlers;
  }

  get state(): ConnectionState {
    return this._state;
  }

  private setState(s: ConnectionState) {
    this._state = s;
    this.handlers.onState?.(s);
  }

  connect(): void {
    if (this._state === "open" || this._state === "connecting") return;
    this.setState("connecting");
    const ws = new WebSocket(wsUrl());
    this.ws = ws;

    ws.onopen = () => this.setState("open");
    ws.onclose = () => {
      if (this.ws === ws) {
        this.ws = null;
        this.setState("closed");
      }
    };
    ws.onmessage = (e) => {
      let frame: ServerFrame;
      try {
        frame = JSON.parse(e.data as string) as ServerFrame;
      } catch {
        return;
      }
      switch (frame.type) {
        case "opened":
        case "switched":
          this.handlers.onSession?.(frame.session_id);
          break;
        case "forked":
          this.handlers.onSession?.(frame.session_id);
          this.handlers.onForked?.(frame.session_id);
          break;
        case "event":
          this.handlers.onEnvelope?.(frame.envelope);
          break;
        case "error":
          this.handlers.onError?.(frame.error);
          break;
        case "notice":
          this.handlers.onNotice?.(frame.text, frame.session ?? "");
          break;
      }
    };
  }

  close(): void {
    this.ws?.close();
    this.ws = null;
    this.setState("closed");
  }

  private send(frame: ClientFrame): boolean {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return false;
    this.ws.send(JSON.stringify(frame));
    return true;
  }

  /** Open a fresh session, or re-bind one by id (resume on reconnect). */
  open(sessionId?: string): boolean {
    return this.send(
      sessionId ? { type: "open", session_id: sessionId } : { type: "open" },
    );
  }

  private intent(intent: Intent): boolean {
    return this.send({ type: "intent", intent });
  }

  prompt(text: string, parts?: ContentBlock[], author?: string): boolean {
    const data: Extract<Intent, { kind: "prompt" }>["data"] = { text };
    if (parts && parts.length > 0) data.parts = parts;
    // The host's self-asserted name for a shared chat. Trusted because the host
    // is a full principal (not a scoped guest, whose name the server overwrites
    // in filterShareFrame); it labels the host's messages for the guests.
    if (author) data.author = author;
    return this.intent({ kind: "prompt", data });
  }

  steer(text: string, parts?: ContentBlock[], author?: string): boolean {
    const data: Extract<Intent, { kind: "steer" }>["data"] = { text };
    if (parts && parts.length > 0) data.parts = parts;
    if (author) data.author = author;
    return this.intent({ kind: "steer", data });
  }

  /** Post a human-to-human side-chat line. NOT a prompt: the server logs and
   *  broadcasts it to the other doors without starting an agent turn. Author is
   *  self-asserted by the host and overwritten server-side for a share guest. */
  chatNote(text: string, author?: string): boolean {
    const data: Extract<Intent, { kind: "chat_note" }>["data"] = { text };
    if (author) data.author = author;
    return this.intent({ kind: "chat_note", data });
  }

  /** Switch the model for this session (server's set_model shorthand). */
  setModel(model: string): boolean {
    return this.send({ type: "set_model", model });
  }

  /** Toggle provider-side web search for this session (set_web_search shorthand). */
  setWebSearch(enabled: boolean): boolean {
    return this.send({ type: "set_web_search", enabled });
  }

  /** Toggle provider-side web fetch for this session (set_web_fetch shorthand). */
  setWebFetch(enabled: boolean): boolean {
    return this.send({ type: "set_web_fetch", enabled });
  }

  /** Toggle provider-side image generation for this session (set_image_gen shorthand). */
  setImageGen(enabled: boolean): boolean {
    return this.send({ type: "set_image_gen", enabled });
  }

  /**
   * Branch the bound session at throughSeq (last event kept; negative keeps
   * nothing) and rebind this connection to the branch. The server answers
   * forked + switched, which route to onSession.
   */
  fork(throughSeq: number): boolean {
    return this.send({ type: "fork", through_seq: throughSeq });
  }

  interrupt(): boolean {
    return this.intent({ kind: "interrupt", data: {} });
  }

  approve(requestId: string, approved: boolean): boolean {
    return this.intent({
      kind: "approval_response",
      data: { request_id: requestId, approved },
    });
  }

  /** Answer (or skip) a pending question form — the ask tool's panel. */
  answerQuestions(
    requestId: string,
    answers: QuestionAnswer[],
    details: string,
    skipped: boolean,
  ): boolean {
    const data: Extract<Intent, { kind: "question_response" }>["data"] = {
      request_id: requestId,
    };
    if (answers.length > 0) data.answers = answers;
    if (details) data.details = details;
    if (skipped) data.skipped = true;
    return this.intent({ kind: "question_response", data });
  }
}

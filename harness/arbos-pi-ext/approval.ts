/** Configured approval failures must never become permission to execute a tool.
 * Only an explicitly unconfigured hook retains standalone pi behavior. Wire
 * decisions follow agentd's approval.rs; parking hands execution back to agentd.
 */
import * as Effect from "effect/Effect";
import * as Schema from "effect/Schema";

const Id = Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(256), Schema.isPattern(/^[^\s\u0000-\u001f\u007f]+$/u));
const Text = Schema.String.check(Schema.isMaxLength(16384));
const Reason = Schema.optional(Schema.NullOr(Text));
const Milliseconds = Schema.Number.check(Schema.isFinite(), Schema.isInt(), Schema.isBetween({ minimum: 1, maximum: Number.MAX_SAFE_INTEGER }));
const Decision = Schema.Literals(["pending", "approved", "rejected", "cancelled", "expired"]);
const Unavailable = Schema.Struct({ ok: Schema.Literal(false), error: Schema.optional(Text) });
const InitialResponse = Schema.Union([
  Unavailable,
  Schema.Struct({ ok: Schema.Literal(true), state: Schema.Literal("allowed"), approvalId: Schema.optional(Id), grantId: Schema.optional(Id) }),
  Schema.Struct({
    ok: Schema.Literal(true), state: Schema.Literal("pending"), approvalId: Id,
    title: Schema.optional(Text), why: Schema.optional(Text), waitMs: Schema.optional(Milliseconds),
    park: Schema.optional(Schema.Boolean), chat: Schema.optional(Text), chatId: Schema.optional(Id),
  }),
]);
const StatusResponse = Schema.Union([
  Unavailable,
  Schema.Struct({ ok: Schema.Literal(true), state: Decision, reason: Reason, decidedBy: Schema.optional(Schema.NullOr(Text)) }),
]);
const ParkResponse = Schema.Union([
  Unavailable,
  Schema.Struct({ ok: Schema.Literal(true), state: Schema.Literal("pending"), parked: Schema.Literal(true), reason: Reason }),
  Schema.Struct({ ok: Schema.Literal(true), state: Decision, parked: Schema.Literal(false), reason: Reason }),
]);

type Initial = typeof InitialResponse.Type;
type Status = typeof StatusResponse.Type;
type Park = typeof ParkResponse.Type;
type Failure = { readonly _tag: "ApprovalUnavailable" } | { readonly _tag: "InvalidApprovalResponse" };

export type ApprovalState =
  | { readonly _tag: "Checking" }
  | { readonly _tag: "Pending"; readonly approvalId: string; readonly deadline: number; readonly reason?: string }
  | { readonly _tag: "Parking"; readonly approvalId: string; readonly reason?: string }
  | { readonly _tag: "Allowed"; readonly source: "standalone" | "policy" }
  | { readonly _tag: "Allowed"; readonly source: "approved"; readonly approvalId: string }
  | { readonly _tag: "Denied"; readonly approvalId: string; readonly decision: "rejected" | "cancelled" | "expired"; readonly reason?: string }
  | { readonly _tag: "Parked"; readonly approvalId: string; readonly reason?: string }
  | { readonly _tag: "BlockedPending"; readonly approvalId: string }
  | { readonly _tag: "Failed"; readonly failure: Failure };

type ActiveState = Extract<ApprovalState, { _tag: "Checking" | "Pending" | "Parking" }>;
type TerminalState = Exclude<ApprovalState, ActiveState>;
type ApprovalEvent =
  | { readonly _tag: "Checked"; readonly response: Initial; readonly now: number; readonly fallbackWaitMs: number }
  | { readonly _tag: "Polled"; readonly response: Status }
  | { readonly _tag: "GraceElapsed" }
  | { readonly _tag: "ParkAnswered"; readonly response: Park }
  | { readonly _tag: "Failed"; readonly failure: Failure };

function terminal(state: ApprovalState): state is TerminalState {
  return state._tag !== "Checking" && state._tag !== "Pending" && state._tag !== "Parking";
}

export function approvalTransition(state: ApprovalState, event: ApprovalEvent): ApprovalState {
  if (terminal(state)) return state;
  if (event._tag === "Failed") return { _tag: "Failed", failure: event.failure };
  if (state._tag === "Checking" && event._tag === "Checked") {
    const response = event.response;
    if (!response.ok) return { _tag: "Failed", failure: { _tag: "ApprovalUnavailable" } };
    if (response.state === "allowed") return { _tag: "Allowed", source: "policy" };
    return { _tag: "Pending", approvalId: response.approvalId, deadline: event.now + (response.waitMs ?? event.fallbackWaitMs) };
  }
  if (state._tag === "Pending" && event._tag === "GraceElapsed") {
    return { _tag: "Parking", approvalId: state.approvalId, reason: state.reason };
  }
  if ((state._tag === "Pending" && event._tag === "Polled") || (state._tag === "Parking" && event._tag === "ParkAnswered")) {
    const response = event.response;
    if (!response.ok) return { _tag: "Failed", failure: { _tag: "ApprovalUnavailable" } };
    const reason = response.reason ?? state.reason;
    if ("parked" in response && response.parked) return { _tag: "Parked", approvalId: state.approvalId, reason };
    if (response.state === "approved") return { _tag: "Allowed", source: "approved", approvalId: state.approvalId };
    if (response.state !== "pending") return { _tag: "Denied", approvalId: state.approvalId, decision: response.state, reason };
    return state._tag === "Pending" ? { ...state, reason } : { _tag: "BlockedPending", approvalId: state.approvalId };
  }
  return { _tag: "Failed", failure: { _tag: "InvalidApprovalResponse" } };
}

interface ApprovalOptions {
  hookUrl: string;
  token: string;
  body: { tool: string; input: unknown; session_id: string; role: string };
  pollMs: number;
  fallbackWaitMs: number;
  onTransition: (state: ApprovalState) => void;
}

const RESPONSE_BYTES = 65536;
const request = Effect.fn("approval.request")(function* <S extends Schema.Constraint>(
  options: ApprovalOptions, path: string, body: unknown, schema: S,
) {
  const response = yield* Effect.tryPromise({
    try: async (signal) => {
      const res = await fetch(`${options.hookUrl}${path}`, {
        method: "POST",
        headers: { "content-type": "application/json", authorization: `Bearer ${options.token}` },
        body: JSON.stringify(body),
        signal: AbortSignal.any([signal, AbortSignal.timeout(10000)]),
      });
      if (!res.ok || res.headers.get("content-type")?.split(";")[0].trim().toLowerCase() !== "application/json" || !res.body) {
        await res.body?.cancel();
        throw new Error("Approval response unavailable");
      }
      const reader = res.body.getReader();
      const chunks: Uint8Array[] = [];
      let size = 0;
      try {
        while (true) {
          const { done, value } = await reader.read();
          if (done) break;
          size += value.byteLength;
          if (size > RESPONSE_BYTES) { await reader.cancel(); throw new Error("Approval response too large"); }
          chunks.push(value);
        }
      } finally {
        reader.releaseLock();
      }
      const bytes = new Uint8Array(size);
      let offset = 0;
      for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
      return JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes)) as unknown;
    },
    catch: (): Failure => ({ _tag: "ApprovalUnavailable" }),
  });
  return yield* Schema.decodeUnknownEffect(schema, { onExcessProperty: "error" })(response).pipe(
    Effect.mapError((): Failure => ({ _tag: "InvalidApprovalResponse" })),
  );
});

const approvalFlow = Effect.fn("approval.check")(function* (options: ApprovalOptions) {
  let state: ApprovalState = options.hookUrl ? { _tag: "Checking" } : { _tag: "Allowed", source: "standalone" };
  const pollMs = Number.isFinite(options.pollMs) && options.pollMs > 0 ? options.pollMs : 1500;
  const fallbackWaitMs = Number.isSafeInteger(options.fallbackWaitMs) && options.fallbackWaitMs > 0 ? options.fallbackWaitMs : 10000;
  while (!terminal(state)) {
    const previous: ActiveState = state;
    const next: Effect.Effect<ApprovalState, Failure> = Effect.gen(function* () {
      if (previous._tag === "Checking") {
        const response = yield* request(options, "/hook/approval", options.body, InitialResponse);
        return approvalTransition(previous, { _tag: "Checked", response, now: Date.now(), fallbackWaitMs });
      }
      if (previous._tag === "Pending") {
        const remaining = previous.deadline - Date.now();
        if (remaining <= 0) return approvalTransition(previous, { _tag: "GraceElapsed" });
        yield* Effect.sleep(Math.min(pollMs, remaining));
        const response = yield* request(options, "/hook/approval-status", { approvalId: previous.approvalId }, StatusResponse);
        return approvalTransition(previous, { _tag: "Polled", response });
      }
      const response = yield* request(options, "/hook/approval-park", { approvalId: previous.approvalId }, ParkResponse);
      return approvalTransition(previous, { _tag: "ParkAnswered", response });
    });
    state = yield* next.pipe(Effect.catch((failure: Failure) => Effect.succeed(approvalTransition(previous, { _tag: "Failed", failure }))));
    if (state._tag !== previous._tag) options.onTransition(state);
  }
  return state;
});

export function checkApproval(options: ApprovalOptions): Promise<TerminalState> {
  return Effect.runPromise(approvalFlow(options));
}

export function approvalVerdict(state: TerminalState): { block: true; reason: string } | undefined {
  switch (state._tag) {
    case "Allowed": return undefined;
    case "Failed": return { block: true, reason: "Nothing ran: the approval service could not confirm permission. Do not bypass approval; try again when the service is available." };
    case "Parked": return { block: true, reason: state.reason ?? "This needs the user's approval; nothing ran." };
    case "Denied":
      if (state.decision === "rejected") return { block: true, reason: `The user rejected this action (approval ${state.approvalId})${state.reason ? `: ${state.reason}` : ""}. Do not retry it unchanged; tell the user.` };
      if (state.decision === "cancelled") return { block: true, reason: `This work was cancelled, so its approval card (${state.approvalId}) was closed and nothing ran. Whatever it was meant to post, send or change is NOT done.` };
    case "BlockedPending":
      return { block: true, reason: `Nothing ran: this needs the user's approval and card ${state.approvalId} is waiting for them. Do not call it again and do not wait for it — say in one short sentence that it needs their approval, then end your turn. When they approve, Arbos runs the call and gives you the result. Whatever it was meant to post, send or change is NOT done yet; never say it was posted or sent.` };
  }
}

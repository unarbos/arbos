import * as Schema from "effect/Schema";

const Id = Schema.Int.check(Schema.isBetween({ minimum: 0, maximum: Number.MAX_SAFE_INTEGER }));
const Offset = Schema.Int.check(Schema.isGreaterThanOrEqualTo(0));
const Key = Schema.Struct({
  start: Offset,
  end: Offset,
  value: Schema.Union([Schema.String, Schema.Null]),
  confident: Schema.Boolean,
  suggested_name: Schema.String,
});

export const TextResponse = Schema.Struct({
  id: Id,
  patterns: Schema.String,
  patterns_twice: Schema.String,
  placeholders: Schema.String,
  placeholders_twice: Schema.String,
  scrub: Schema.String,
  scrub_twice: Schema.String,
  keys: Schema.Array(Key),
});
export const JsonResponse = Schema.Struct({ id: Id, redact_json: Schema.Json });
export const UrlResponse = Schema.Struct({
  id: Id,
  hinted: Schema.String,
  plain: Schema.String,
  breakable: Schema.String,
});

export type TextReply = typeof TextResponse.Type;
export type JsonReply = typeof JsonResponse.Type;
export type UrlReply = typeof UrlResponse.Type;
export type Request = { op: "text" | "url"; text: string } | { op: "json"; value: Schema.Json };
export type Reply = TextReply | JsonReply | UrlReply;

const decoders = {
  text: Schema.decodeUnknownSync(TextResponse, { onExcessProperty: "error" }),
  json: Schema.decodeUnknownSync(JsonResponse, { onExcessProperty: "error" }),
  url: Schema.decodeUnknownSync(UrlResponse, { onExcessProperty: "error" }),
};

export function decodeReply(op: Request["op"], value: unknown): Reply {
  return decoders[op](value);
}

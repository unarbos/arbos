import { isDeepStrictEqual } from "node:util";
import * as Arbitrary from "effect/Arbitrary";
import * as Effect from "effect/Effect";
import * as Schema from "effect/Schema";
import { Bridge } from "./bridge.ts";
import type { TextReply, JsonReply, UrlReply } from "./protocol.ts";

// Scalar Unicode only, excluding the two reserved rendering hints. Mapping keeps Schema shrinking.
const scalar = Schema.Union([
  Schema.Int.check(Schema.isBetween({ minimum: 0, maximum: 0x034e })),
  Schema.Int.check(Schema.isBetween({ minimum: 0x0350, maximum: 0x200a })),
  Schema.Int.check(Schema.isBetween({ minimum: 0x200c, maximum: 0xd7ff })),
  Schema.Int.check(Schema.isBetween({ minimum: 0xe000, maximum: 0x10ffff })),
]);
const points = Schema.Array(scalar).check(Schema.isMaxLength(32));
const hex = Schema.Array(Schema.Literals(["0", "1", "2", "7", "a", "b", "c", "d", "e", "f"]))
  .check(Schema.isMinLength(32), Schema.isMaxLength(64));
const extended = Schema.Array(Schema.Literals(["0", "A", "b", "-", "_"]))
  .check(Schema.isMinLength(32), Schema.isMaxLength(64));
const input = Schema.Struct({
  prefix: points, suffix: points, body: hex, extended,
  variant: Schema.Literals(["alphanumeric", "mixed", "hyphens", "underscores"]),
});
type Input = typeof input.Type;
const unicode = (values: readonly number[]) => String.fromCodePoint(...values);
const body = (value: Input) => value.body.join("");

/** All 17 families; each body uses only its family's source-permitted alphabet. */
export function tokens(value: Input): string[] {
  const h = body(value);
  const wide = value.variant === "hyphens" ? "-".repeat(32) : value.variant === "underscores" ? "_".repeat(32)
    : value.variant === "mixed" ? value.extended.join("") : h;
  return [
    `sk_live_${h}`, `rk_test_${h}`, `sk-or-v1-${h}`, `sk-ant-${wide}`, `sk-proj-${wide}`, `sk-${h}`,
    `ghp_${h}`, `github_pat_${wide.replaceAll("-", "_")}`, `xoxb-${wide.replaceAll("_", "-")}`, `AKIA${h.slice(0, 16).toUpperCase()}`,
    `ops_${h}`, `dp.st.${wide}`, `dtn_${h}`, `AIza${wide}`, `SG.${wide}.${wide}`, `key-${h.slice(0, 32)}`,
    `eyJ${wide}.${wide}.${wide}`,
  ];
}

const canonical: Input = { prefix: [], suffix: [], body: Array<"0">(32).fill("0"), extended: Array<"0">(32).fill("0"), variant: "mixed" };
const cases = Arbitrary.schema(input, {
  shrink: (value) => JSON.stringify(value) === JSON.stringify(canonical) ? [] : [canonical],
});
const textFor = (value: Input) => `${unicode(value.prefix)} λ😀 ( ${tokens(value).join(" )\nλ😀 ( ")} ) ${unicode(value.suffix)}`;

function text(bridge: Bridge, value: string) {
  return Effect.tryPromise({
    try: async () => await bridge.request({ op: "text", text: value }) as TextReply,
    catch: () => new Error("Native text oracle failed"),
  });
}

function validByteRanges(value: string, reply: TextReply): boolean {
  const boundaries = new Set([0]);
  let offset = 0;
  for (const scalar of value) {
    offset += Buffer.byteLength(scalar);
    boundaries.add(offset);
  }
  const bytes = Buffer.from(value);
  let previousEnd = 0;
  return reply.keys.every((key) => {
    const valid = key.start >= previousEnd && key.start < key.end && boundaries.has(key.start) && boundaries.has(key.end)
      && key.value !== null && bytes.subarray(key.start, key.end).toString("utf8") === key.value;
    previousEnd = key.end;
    return valid;
  });
}

function sameStructure(before: Schema.Json, after: Schema.Json): boolean {
  if (typeof before === "string") return typeof after === "string";
  if (before === null || typeof before !== "object") return Object.is(before, after);
  if (Array.isArray(before)) {
    return Array.isArray(after) && before.length === after.length
      && before.every((item, i) => sameStructure(item, after[i]));
  }
  if (after === null || typeof after !== "object" || Array.isArray(after)) return false;
  const a = before as Record<string, Schema.Json>;
  const b = after as Record<string, Schema.Json>;
  const keys = Object.keys(a).sort();
  return JSON.stringify(keys) === JSON.stringify(Object.keys(b).sort())
    && keys.every((key) => sameStructure(a[key], b[key]));
}

export const laws = [
  {
    name: "redaction-idempotence", seed: 81001, arbitrary: cases,
    property: (value: Input, bridge: Bridge) => Effect.map(text(bridge,
      `${textFor(value)}\nAuthorization: Bearer synthetic.${body(value)}\nAPI_KEY=synthetic-${body(value)}\nop://Synthetic/item arbos-secret://synthetic $ARBOS_SECRET_EXAMPLE`),
    (reply) => reply.patterns === reply.patterns_twice && reply.placeholders === reply.placeholders_twice && reply.scrub === reply.scrub_twice),
  },
  {
    name: "confident-token-removal", seed: 81002, arbitrary: cases,
    property: (value: Input, bridge: Bridge) => Effect.map(text(bridge, textFor(value)), (reply) => {
      const recognized = reply.keys.filter((key) => key.confident).map((key) => key.value);
      return tokens(value).every((token) => recognized.includes(token)
        && !reply.patterns.includes(token) && !reply.scrub.includes(token));
    }),
  },
  {
    name: "utf8-key-byte-ranges", seed: 81003, arbitrary: cases,
    property: (value: Input, bridge: Bridge) => Effect.map(text(bridge, textFor(value)), (reply) =>
      reply.keys.some((key) => key.confident) && validByteRanges(textFor(value), reply)),
  },
  {
    name: "json-scrub-compositionality", seed: 81004, arbitrary: cases,
    property: (value: Input, bridge: Bridge) => Effect.tryPromise({
      try: async () => {
        const token = tokens(value)[3];
        // Normalize through the actual wire JSON domain before comparing numbers/structure.
        const original = JSON.parse(JSON.stringify({
          [unicode(value.prefix) + '"quoted\\keyλ']: [token, { nested: unicode(value.suffix), zero: -0, enabled: true, absent: null }],
          [token]: { value: token, number: value.body.length, disabled: false },
        })) as Schema.Json;
        const reply = await bridge.request({ op: "json", value: original }) as JsonReply;
        if (!sameStructure(original, reply.redact_json)) return false;
        const expected = async (v: Schema.Json): Promise<Schema.Json> => {
          if (typeof v === "string") return (await bridge.request({ op: "text", text: v }) as TextReply).scrub;
          if (v === null || typeof v !== "object") return v;
          if (Array.isArray(v)) {
            const values: Schema.Json[] = [];
            for (const item of v) values.push(await expected(item));
            return values;
          }
          const entries: [string, Schema.Json][] = [];
          for (const [key, item] of Object.entries(v)) entries.push([key, await expected(item)]);
          return Object.fromEntries(entries);
        };
        return isDeepStrictEqual(reply.redact_json, await expected(original));
      },
      catch: () => new Error("Native JSON oracle failed"),
    }),
  },
  {
    name: "url-visible-text-roundtrip", seed: 81005, arbitrary: cases,
    property: (value: Input, bridge: Bridge) => Effect.tryPromise({
      try: async () => {
        const original = `${unicode(value.prefix)} (https://例.invalid/路径/a/b?x=${body(value)}&y=2), http://a.invalid/p; ${unicode(value.suffix)}`;
        const reply = await bridge.request({ op: "url", text: original }) as UrlReply;
        return reply.plain === original && reply.hinted.includes("\u034f") && reply.hinted.includes("\u200b");
      },
      catch: () => new Error("Native URL oracle failed"),
    }),
  },
] as const;

// This broader boundary is intentionally separate: baseline trailing-word-boundary policy violates it.
const adversarial = Schema.Struct({
  family: Schema.Literals(["anthropic", "openai", "slack", "doppler", "google"]),
  body: hex,
});
export const boundaryLaw = {
  name: "confident-token-nonword-ending-removal", seed: 81006,
  arbitrary: Arbitrary.schema(adversarial, {
    // Literal choices have no semantic simplification; retain one valid minimal failing family/body.
    shrink: (value): ReadonlyArray<typeof adversarial.Type> => value.family === "anthropic" && value.body.every((c) => c === "0") ? []
      : [{ family: "anthropic", body: Array<"0">(32).fill("0") }],
  }),
  property: (value: typeof adversarial.Type, bridge: Bridge) => {
    const spec = { anthropic: ["sk-ant-", 20], openai: ["sk-proj-", 20], slack: ["xoxb-", 10], doppler: ["dp.st.", 10], google: ["AIza", 30] } as const;
    const [prefix, minimum] = spec[value.family];
    const token = prefix + value.body.slice(0, minimum - 1).join("") + "-";
    return Effect.map(text(bridge, `λ😀 (${token})`), (reply) => reply.keys.some((key) => key.confident && key.value === token)
      && !reply.patterns.includes(token) && !reply.scrub.includes(token));
  },
};

export function checkLaw<A>(law: { arbitrary: Arbitrary.Arbitrary<A>; property: (value: A, bridge: Bridge) => Effect.Effect<boolean, Error>; seed: number }, bridge: Bridge, replay?: string) {
  return Effect.runPromise(Arbitrary.checkEffect(law.arbitrary, (value) => law.property(value, bridge), {
    runs: 256, size: 64, seed: law.seed, maxShrinks: 256, concurrency: 1, ...(replay ? { replay } : {}),
  }));
}

const safeText = Arbitrary.map(Arbitrary.schema(Schema.Array(Schema.Literals([
  "a", "B", "é", "λ", "😀", " ", "\n", ".", ",", "!", "(", ")", "1", "-",
])).check(Schema.isMaxLength(128))), (characters) => characters.join(""));

export const nonsecretLaw = {
  name: "nonsecret-text-preservation", seed: 81007, arbitrary: safeText,
  property: (value: string, bridge: Bridge) => Effect.map(text(bridge, value), (reply) =>
    reply.patterns === value && reply.placeholders === value && reply.scrub === value),
};

function runnable<A>(law: Parameters<typeof checkLaw<A>>[0] & { name: string }) {
  return { name: law.name, seed: law.seed, check: (bridge: Bridge, replay?: string): Promise<Arbitrary.CheckResult<unknown, Error>> => checkLaw(law, bridge, replay) };
}

export const requiredLaws = [...laws.map((law) => runnable(law)), runnable(boundaryLaw), runnable(nonsecretLaw)];

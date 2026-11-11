# Phase 6: native client, Matrix and FFI hot paths

Read-only scan based on `2865cb29b806121d4a90f55ba6ce151a150279a0`, with
Phase 1/3/4 findings carried forward. Coverage is the 199-file Swift inventory,
125 Mac Rust source files, shared Matrix client and Rust FFI. Broad searches cover
all required categories; focused code reads substantiate the opportunities below.
This is not a claim to have profiled every source line or a measured diagnosis of
the reported hosted-app symptoms. No native build, app/device launch, hosted call,
user database, credential or secret was used by this subtask.

Path aliases: **I** = `apps/ios/Arbos/Arbos/`; **M** =
`crates/arbos-mac/src/`; **X** = `crates/arbos-matrix/src/`; **F** =
`crates/arbos-ffi/src/`.

## Required category disposition

| Category | Code evidence | Disposition |
| --- | --- | --- |
| Independent sequential async | Mac registry `M/sync/relay.rs:2585` serially fetches unread/projects, then detail/members per project before people `/chats`. iOS startup `I/Store/AppModel.swift:1853` serializes registry/account/people/server/Matrix/pushes/unread; attachment uploads at `:3657,5543` and `M/sync/relay.rs:4900` are one at a time. | Real candidates for bounded concurrency after ownership/cancellation tests. Not all sequential awaits are independent: preserve credential refresh, room creation dependencies, ordered per-chat sends, blob order and error semantics. Native changes blocked pending regression gates. |
| Whole database rows for a few fields | No `SELECT *` found in the scoped native/Matrix/FFI code. Device caches use `SELECT v ... WHERE k` for full consumed JSON; `load_docs` selects key/project/room/cursor/snapshot and consumes them (`M/sync/store.rs:229`, `I/Store/DeviceStore.swift:198`). Usage/eviction queries select only counts, byte lengths or key/bytes (`:268–291`). | No demonstrated projection fix. Loading all saved doc blobs at startup is a scheduling/working-set opportunity, not redundant columns. Matrix SDK store SQL is not owned here; no speculative query changes there. |
| Unnecessary URL parsing | iOS `SenderAccents.load` (`I/UI/Transcript.swift:2764`) constructs URL before the requested-set cache hit guard. Normal avatar cache `:2916` checks cached/inflight first, then parses once. Markdown URL objects are constructed only on cached-parser misses (`:1344,1371`). RelayAPI parses request URLs at request construction (`:128`); auth/callback/call URLs are meaningful trust/SDK boundaries. Mac display URLs commonly use string handling rather than `Url::parse`. | Guard ordering is a minor candidate; no demonstrated native measurement to justify a standalone patch. Do not remove URL validation or replace origin/host semantics with string prefixes. Query/width transformations in AvatarImageCache.sizedURL are required. |
| Static regex rebuilt in functions/loops | Broad explicit-constructor and regex-reference search found 13 constructor hits, all in XCTest source-lint tests; most are static patterns. No runtime Regex::new/NSRegularExpression construction found in native app, Mac, Matrix or FFI sources. SearchIndex's NSDataDetector is already `private static let` (`I/Model/SearchIndex.swift:49`). | No runtime regex-hoisting patch for this scope. Tests compiling several lint patterns per test are not the group-open/send hot path. Shared-core/TypeScript regex work is owned by the other Phase 6 agents. |
| Single key from large jsonb | No jsonb/PostgreSQL query or JSON SQL extraction in native device stores. Mac `emit_project_doc` deep-projects the whole Loro memory map to read `t1` (`M/sync/relay.rs:3145`); iOS projects settings/status maps to read modelNote/workspace (`I/Store/AppModel.swift:2847`). | Potential narrower Loro/FFI projection if measured large, but not a jsonb-column migration. Persistent cache registry JSON is read to populate its whole list, so no unjustified column/backfill change. |

## User-visible paths and what must be measured

People DMs and groups use the same Loro room transport on the inspected base.
Group differences include larger histories, member-name/avatar/addressing work,
and agent reply selection after delivery. Matrix serves server channels here;
improving Matrix channel snapshot work does not by itself prove faster Loro
people-group open.

Keep these measurements distinct:

1. navigation tap → cached transcript/first visible row;
2. first visible row → scrollable frame budget;
3. composer tap → local optimistic row;
4. local row → durable relay ACK / remote-human delivery;
5. human send → agent decision / first agent token.

An existing loading indicator, faint pending bubble and slow agent response can
have different causes. No seconds-to-load claim is supported by static source
inspection alone.

## Prioritized opportunities

### 1. Mac full snapshot and map-lock contention

`M/sync/relay.rs:3014` holds the global docs-map mutex while decoding every message
through Loro→serde JSON→native Message, then checks pending state per message.
The same lock guards imports, exports and send transactions. Live list changes in
`forward_changes` (`:3293`) generate full snapshots; catch-up alone is coalesced.

After snapshot arrival, `M/app.rs:2964` replaces full messages, reindexes the chat,
walks arrivals, requests previews and clones messages for icon work. The transcript
cache avoids rebuilding unchanged rows but still fingerprints input messages.
`M/ui/transcript.rs:4223` allocates Debug strings for each Part/turn/replyRef/job
while hashing every message's render version. A warm row cache is therefore not
free for large histories or large hidden tool content.

Measure separately: global lock wait/hold; import; row projection; event queue
delay; AppState index/update; renderer fingerprint and row-build work. Prefer
short map-lock lookup and room-owned handles, then more granular snapshot/upsert
events. Do not introduce concurrency while the Phase 4 stale-room/account and
pending-ACK ownership regressions remain unpinned.

### 2. Registry hydration gates people-chat readiness

Mac `refresh_registry` waits for each project's independent detail/member request
before fetching `/chats`; one slow project therefore delays unrelated people
groups. iOS similarly waits registry/account before people refresh and awaits a
machine refresh before starting new project rooms. Initial no-cache launch and
many-project accounts need separate measurements from warm tab switching.

Use a bounded test HTTP fixture with 0/10/100 projects and deterministic delays;
assert identical preserved/skipped projects, deterministic order, current-account
ownership and per-request failures before measuring critical-path improvement.
Existing core DTO, skipped-project merge and registry tests are useful seams.

### 3. Multi-attachment upload and intentional per-chat FIFO

Mac and iOS upload independent attachments sequentially. A bounded ordered result
collection can reduce a multi-file send's network critical path, while one failed
file still reports the same error. Mac's `ChatSendQueue` deliberately preserves
photo-before-text send order; it must not be removed to make one following text
appear faster. Separate upload time, queue wait and row commit/ACK.

The iOS older-relay directory fallback instead starts 36 requests concurrently
(`AppModel.swift:3178`). Bound it if measurements show it contends with chat
requests. The ordinary cached `/users` path already avoids that fan-out.

### 4. iOS startup and persistence working set

`I/Store/AppModel.swift:2057` loads/imports/decodes all cached docs in one detached
task before returning any result to MainActor. Last-opened-first sorting does not
stream the first restored room independently. `flushStore` at `:2187` synchronously
exports complete dirty documents on MainActor before queueing SQLite writes.

Potential improvement: stage currently open/recent rooms, then background results
with explicit room/account read tickets. Preserve cursor-before-export durability,
pending batches, redaction-aware snapshots, caps and failed-read recovery. Phase 4
identified a stale detached-read publication hazard; launching more work without
epoch-aware regressions would make that lifecycle harder to reason about.

### 5. Matrix snapshot/timeline scaling (channels)

`X/client.rs:303` iterates joined rooms and awaits metadata, child order or power
levels per room. Some SDK operations read cached local state, so these are not
necessarily one network round trip each. Measure before treating every await as
remote latency. `names_for` at `:389` deduplicates successful names but serially
looks up each unique sender; first missing membership can fetch the whole room
roster. Blindly parallelizing misses can duplicate that fetch.

`push_rows` at `:415` holds a global timeline map mutex, checks each new event
against the whole vector (`any`), sorts the whole history on changes and clones
the whole vector even for a cached `open_room`. Notification adds another clone.
An event-id membership index and ordered merge could improve history scaling, but
must preserve duplicate-event, local-id echo, equal-timestamp ordering and
pagination behavior. The page size is already bounded; accumulated open-room
history is not capped by that page size alone. FFI copies full row lists into
Swift, multiplying projection costs. Native/SDK regression prerequisites block a
safe implementation in this pass.

### 6. Pure URL hint scanning is independently measurable

`M/ui/urlwrap.rs:67` decorates literal URLs for the pinned GPUI line wrapper. Its
`find_url_start` at `:84` separately searches the entire suffix for `https://` and
`http://` on each URL iteration. For many HTTPS links and no HTTP link, the absent
scheme search scans every remaining suffix, creating quadratic work in link
count. A single scan of `http` candidates followed by scheme-prefix validation
should preserve the earliest valid occurrence and make scanning linear.

This is used by actual bubble rendering at `M/ui/transcript.rs:5475,5558,5768` and
markdown URL layout at `M/ui/markdown.rs:235`; keep all existing invisible
wrap/hold characters and copy-paste round-trip behavior unchanged.

The runtime functions are std-only, but the same module's test block also has
GPUI visual tests, so `rustc --test urlwrap.rs` **cannot** run unchanged. A normal
standalone Rust benchmark driver can include the actual production module with
`cfg(test)` off and measure 1/100/1000-link inputs without native dependencies.
Root approved this small exception to the read-only scan. Only `find_url_start`
changed, to walk `http` candidates once and validate the two existing prefixes.
All wrapping characters, URL termination rules and existing GPUI tests remain
unchanged.

The retained driver in `.improve/perf/urlwrap/` pinned the actual original module
before editing: 10 fixed output goldens plus 125 generated visible-text round
trips passed. The same checks passed against the changed module. It also ran
from `/tmp`, proving repository-path independence. The optimized public
`hint_bare_urls` operation was measured in 31 alternating before/after pairs for
HTTPS, HTTP and mixed inputs at 1, 100 and 1,000 links. At 1,000 links, median
HTTPS work fell from 2,004.850 to 134.608 microseconds (14.89×), HTTP from
3,108.083 to 132.083 microseconds (23.53×), and mixed from 177.275 to 141.250
microseconds (1.26×). One-link medians improved from 0.219–0.233 to 0.156–0.161
microseconds. `.improve/perf/urlwrap/README.md` describes inputs, limitations and
reproduction; `results.json` contains raw samples and source hashes. This is a
long-message formatting result, not evidence that ordinary chat-open/send or
visual frame latency improved. GPUI/full native gates remain blocked.

## Existing optimizations to preserve

- Shared sans-IO RoomSession across Mac and iOS via FFI; cursor/checkpoint/pending
  semantics are central, not extra call-site protocol work to remove.
- iOS one bulk FFI read + JSON parse (`CoreChatDoc.swift:82`) instead of one full
  projection per row; malformed-row fallback protects nonempty transcripts.
- iOS catch-up coalescing 600 ms, live bursts 150 ms, background decoding from
  300 messages (`AppModel.swift:924–933`) and incremental single-message updates.
  These are recorded owner measurements/requirements, not new audit timings.
- iOS per-chat message versions and COW storage identity (`AppModel.swift:318–355`)
  avoid array comparisons per view body; row storage equality has a pointer fast
  path (`Transcript.swift:269`). ChatView refreshRows uses unchanged/tail/append
  paths and an 80-row initial / 360-row maximum realization window.
- iOS block/inline markdown NSCaches (`Transcript.swift:1239,1344`) and search
  index per-chat version gating (`Model/SearchIndex.swift:66`).
- Mac GPUI ListState virtualizes rows, MessageRowCache retains unchanged rows,
  append-only snapshot arrivals avoid rebuilding a full seen-id set, and jump to
  latest lays out the newest page instead of spring-crawling through history.
- Avatar downloads share in-flight waiters, bound warm-ahead to 48, request
  display-sized images and retain versioned caches. Physical scale/resampling and
  scroll geometry calibrations stay intact.

## Verification plan / honest boundary

Existing runnable-after-prerequisites checks: iOS OptimisticSendTests (group/DM
local paint/no duplicate), LongTranscriptPerfTests (2,000-row bulk decode and 5,000
mixed rows), RowStorageTests (incremental splice equals full build),
BlankTranscriptGuardTests, SessionRoomTests; Mac ChatSendQueue tests, transcript
stream/cache tests (`:12254–12306`) and jump-to-latest viewport tests (`:9005`).
Debug iOS `perfReplay` feeds local checkpoint/row fixtures through real onChange
and snapshot paths; SendTrace marks tap→commit→row→ACK; PerfSignpost marks row build
and navigation. Debug simulator times are not release-device frame/hitch results:
`PerfLog.swift:1–13` explicitly calls for Release Instruments on a device.

Full native lint/typecheck/test/build and device performance measurements remain
blocked by local scratch capacity and unapproved fork CI. The standalone URL
formatter is the one approved, measured native-code exception above. All other
native before/after timings in this scan are **unmeasured**, not zero, unchanged
speed or proof of improvement. No index/migration is proposed for device stores
without query-plan measurements. This inventory supplies real leads while shared
core and TypeScript measurements proceed independently.

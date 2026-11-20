# Metrics

Prometheus metrics for the relay and for each box's agentd. The code is `metrics.rs` in
`crates/arbos-relay/src` and in `crates/arbos-agentd/src`; a test in each fails when a family is
missing from this page.

## Scraping

| what | value |
|---|---|
| address | `http://<ARBOS_RELAY_METRICS_LISTEN>/metrics`, e.g. `http://127.0.0.1:9108/metrics` |
| per box | `http://<ARBOS_RELAY_METRICS_LISTEN>/metrics/boxes`, see [Boxes](#boxes) |
| auth | none; the listener is loopback-only |
| format | Prometheus text exposition 0.0.4 |

The endpoint is on a listener of its own, not on the relay's main port. `ARBOS_RELAY_METRICS_LISTEN`
must be a loopback socket address (`127.0.0.1:PORT`). On anything else, or when the port is taken,
the relay logs `metrics listener is off` and runs without the endpoint: a mistake here costs the
dashboards, never the relay. With the variable unset there is no endpoint at all. Caddy proxies only to the main listener, so
the internet cannot reach it, and a box is on a bridge network, so a box cannot either. Prometheus
has to run on the relay host itself (a host service, or a container with host networking).

`scripts/deploy/relay-server.sh` keeps the variable from `/etc/arbos/relay.env` across deploys.

### Boxes

Prometheus does not talk to a box. The relay's metrics listener lists the awake boxes and proxies
each scrape, because only the relay knows a box's port and token:

| what | value |
|---|---|
| discovery | `http://127.0.0.1:9108/metrics/targets` (Prometheus `http_sd_configs`) |
| each target | address = the listener itself, `__metrics_path__` = `/metrics/box/<id>`, label `box` = `<id>` |
| auth | none on the Prometheus side; the relay sends the box's own bearer to agentd's `GET /metrics` |

```yaml
- job_name: arbos-agentd
  http_sd_configs:
    - url: http://127.0.0.1:9108/metrics/targets
```

`<id>` is the same keyed id as on `/metrics/boxes` (below): 12 hex characters, an HMAC of the
box's owner under a key derived from the relay key, stable across restarts. On its own it cannot
be turned into a user without that key. It is not anonymous to everyone, though: a Prometheus
that also scrapes `/metrics/boxes` holds `arbos_relay_box_info`, which joins the id to the box's
container name, and that name contains the user id. Treat the agentd series like the
`/metrics/boxes` series: for the team's own dashboards, not for sharing outside.

A box is its user's machine, so the relay does not trust what one answers:

- It asks only a box that is awake right now, and answers 404 for any other id. A sleeping box
  keeps its last address, which may since belong to another box.
- It follows no redirect: the request carries the box's token.
- It reads at most 256 KB.
- It passes on only the samples in the agentd table below, by exact name, with that family's one
  label and one of that label's listed values, each series once, with a numeric value. Comment
  lines are not passed on. Anything else is dropped and counted in
  `arbos_relay_box_scrape_dropped_lines_total`.
- `arbos_agentd_build_info` is not taken from the box at all: the relay writes it, with the
  version of the image it gave that box (the box row's snapshot, `arbos-agent-<version>`, so a
  pre-release such as `0.7.0-rc1` reads as it is). A version a box reports about itself is a
  string of its own choosing; this one is bounded by the images ever rolled.

So a box cannot add a family, a label or a label value, and its answer is at most about forty
series. Set `sample_limit` and `body_size_limit` on the Prometheus job as well.

**Do not add the `arbos-agentd` job to the monitoring config before the box image is rolled.** The
relay half is live as soon as the relay is deployed, but a box on an older image has no
`/metrics`: every box would read down (`502`, `old_image`).

Use the listener's IP address in the discovery URL
(`127.0.0.1`, not `localhost`): the relay hands that address back as the scrape target. Only awake
boxes are listed, so a sleeping box is absent, not down. A listed box that answers `502` is
unreachable, runs an image from before agentd had `/metrics`, or answered something the relay
rejected; `arbos_relay_box_scrapes{result="unreachable"|"old_image"|"rejected"}` on the
relay says which, and how many.

A scrape never waits on the relay: gauges are read from memory with `try_lock` (a busy map leaves its
series out of that one scrape), and the database numbers come from a sampler that runs every 10 s.

## Labels

Label values are bounded sets: route template, status class, box state, provider, result, refusal
code, version. No series on `/metrics` carries a user, chat, room, project, token or address, or
anything per box; those are on `/metrics/boxes` only. `route` is the
route template (`/chats/{id}/messages`), never the path; a request that matches no route is
`route="unmatched"`.

## Relay

| name | type | labels | meaning |
|---|---|---|---|
| `arbos_relay_build_info` | gauge | `version` | always 1 |
| `arbos_relay_uptime_seconds` | gauge | | seconds since start; a restart shows as a drop |
| `arbos_relay_rooms_open` | gauge | | sync rooms held in memory |
| `arbos_relay_sockets` | gauge | `kind` = `client` `box` | open room websockets |
| `arbos_relay_boxes` | gauge | `state` = `awake` `waking` `asleep` `error` | boxes by the state the relay believes. A box the relay has not heard of since it started is in no state yet |
| `arbos_relay_box_scrapes` | gauge | `result` = `ok` `old_image` `unreachable` `rejected` `pending` | awake boxes by how their last agentd scrape through the relay ended. `old_image`: the box answered but has no `/metrics` yet, so it shows no agentd series until it is re-imaged. `rejected`: the answer was over 256 KB or not text. `pending`: awake and not asked in the last 5 minutes, which is every awake box while the per-box scrape job is off; it is not a failure and no rule counts it as one. A result is only held for those 5 minutes, so a job that was on and is switched off again stops reading as boxes failing |
| `arbos_relay_box_scrape_dropped_lines_total` | counter | | lines in a box's answer that are not agentd's and were not passed on; above zero means a box is sending something it should not |
| `arbos_relay_boxes_registered` | gauge | `provider` | box rows in the database |
| `arbos_relay_boxes_max` | gauge | | resident box cap (`max_boxes`) |
| `arbos_relay_boxes_turn_running` | gauge | | boxes whose keepalive said a turn is running in the last 3 min |
| `arbos_relay_boxes_evicted` | gauge | | boxes stopped under host pressure, not yet woken by their user |
| `arbos_relay_boxes_upgrading` | gauge | | box upgrades in progress |
| `arbos_relay_boxes_creating` | gauge | | box creates in progress |
| `arbos_relay_waitlist_length` | gauge | | users waiting for a box slot |
| `arbos_relay_wake_queue_depth` | gauge | | wake jobs queued or in flight |
| `arbos_relay_box_starts_total` | counter | `result` = `ok` `fetching` `failed` | provider start attempts by the wake runner. `fetching` is a cold box still coming back, not a failure |
| `arbos_relay_box_start_duration_seconds` | histogram | | time the provider took to start a box, successful starts only |
| `arbos_relay_box_wake_duration_seconds` | histogram | | wake started → the box connected back; what a waiting user feels |
| `arbos_relay_room_pushes_total` | counter | `result` = `stored` `dup` `refused` | room pushes |
| `arbos_relay_room_push_refusals_total` | counter | `code` | refused room pushes by refusal code (`too_large`, `rate_limited`, `not_available`, the push guard's codes) |
| `arbos_relay_room_checkpoints_total` | counter | `result` | checkpoint requests from boxes: `stored`, `too_soon` (inside the 30 s per-room interval), `upload_incomplete` (the box's replica lacks rows), `room_incomplete` (the relay holds a row its own doc cannot apply; that room does not compact), `below_floor`, `error` |
| `arbos_relay_notifications_total` | counter | `result` = `enqueued` `deduped` `delivered` `retried` `dropped_expired` `dropped_permanent` `dropped_no_token` | device notifications (APNs) |
| `arbos_relay_model_requests_total` | counter | `provider` = `openrouter` `anthropic` `openai` `other`, `class` = `2xx`…`5xx` `unreachable` | upstream model calls, by the provider the relay sent them to |
| `arbos_relay_model_response_seconds` | histogram | | time for the provider to answer with response headers (not the whole stream) |
| `arbos_relay_tokens_total` | counter | `kind` = `input` `output` `cache_read` `cache_write` | tokens the proxy metered, every caller included. The four kinds are disjoint (`input` is uncached input). Counted when a call ends, because the provider reports usage last; present at 0 from the start |
| `arbos_relay_spend_today_usd` | gauge | | relay-wide debits since 00:00 UTC, machine cost excluded; what the kill switch compares with the cap |
| `arbos_relay_spend_daily_cap_usd` | gauge | | global daily spend cap (`ARBOS_GLOBAL_DAILY_CAP_USD`) |
| `arbos_relay_kill_switch` | gauge | | 1 while the kill switch is on and every spend is refused. Read only: an automatic trip from an earlier UTC day reads 0 without the stored switch being touched |
| `arbos_relay_http_requests_total` | counter | `route`, `class` = `1xx`…`5xx` | HTTP requests. A websocket upgrade is `1xx` |
| `arbos_relay_http_request_seconds_total` | counter | `route`, `class` | seconds spent until the response headers; divide by the request rate for a mean. An upgrade or a stream counts only its start |
| `arbos_relay_db_bytes` | gauge | `file` = `db` `wal` | size of `relay.db` and its write-ahead log on disk. The WAL sits near 4 MB in normal running; one that stays well above 64 MB means a read is being held open across many commits |
| `arbos_relay_db_writer_wait_seconds` | histogram | | how long a caller waits for the database writer lock, probed every 10 s |
| `process_resident_memory_bytes` | gauge | | resident memory (Linux) |
| `process_open_fds` | gauge | | open file descriptors (Linux) |
| `process_cpu_seconds_total` | counter | | user + system CPU time (Linux) |
| `process_start_time_seconds` | gauge | | start time, seconds since the epoch |

Every relay histogram uses the same buckets, in seconds: 0.001, 0.005, 0.025, 0.1, 0.25, 0.5, 1, 2.5, 5,
10, 30, 60, 120, 300.

Counters and histograms start at zero when the relay starts. A series with labels appears with its
first event.

## Boxes

`GET /metrics/boxes`, on the same loopback listener, has one set of series per registered box. It
is a path of its own so that a Prometheus which scrapes `/metrics` holds nothing per box, whatever
it is later configured to share: the per-box series are for the team's own dashboards.

`box` is a keyed id: the first 12 hex characters of HMAC-SHA256 over the user id, with a key
derived from the relay key for this purpose only. It is stable across restarts and cannot be
derived from a user id without the relay key (user ids are public, so a bare hash could be).
`arbos_relay_box_info` is the one series that says which box is which: its `container` label is
the box's container name, which contains the user id, so that these series can be joined to
cAdvisor's. Scrape this path only into a Prometheus whose readers may see user ids.

| name | type | labels | meaning |
|---|---|---|---|
| `arbos_relay_box_info` | gauge | `box`, `container` | always 1; absent for a box that has no container yet |
| `arbos_relay_box_state` | gauge | `box`, `state` = `awake` `waking` `asleep` `error` `unknown` | always 1, one series per box. `unknown`: the relay has not heard of the box since it started |
| `arbos_relay_box_turn_running` | gauge | `box` | 1 while the box's keepalive said a turn is running in the last 3 min |
| `arbos_relay_box_sockets` | gauge | `box` | open room websockets of the box |
| `arbos_relay_box_tokens_total` | counter | `box`, `kind` | tokens metered for the box's own model calls (calls made with its box token), by the kinds above |
| `arbos_relay_box_model_calls_total` | counter | `box` | model calls of the box that ended and were metered, failed ones included |
| `arbos_relay_box_spend_usd_total` | counter | `box` | metered cost of the box's successful model calls: the amount passed to the ledger for inference, before any clamp at the balance. Voice and images are not in it |

A box's series exist while its row does: every registered box is listed, a box that has called
nothing reads 0, and a deleted box leaves within 10 s. That bounds the page at ten series per box.
The counters start at zero with the relay and are counted in memory where the proxy already meters
the call, after the call has been charged; nothing is read from or written to the database for
them. State, turn running and sockets are read from the relay's live maps without waiting: when
one is busy, a box keeps what the scrape before read.

## agentd

One set of series per box (label `box`, see above).

| name | type | labels | meaning |
|---|---|---|---|
| `arbos_agentd_build_info` | gauge | `version` | always 1. Through the relay, `version` is the image the relay gave the box, not what the box says it runs |
| `arbos_agentd_uptime_seconds` | gauge | | seconds since agentd booted |
| `arbos_agentd_workspaces` | gauge | `health` = `healthy` `unhealthy` | workspaces on the box; unhealthy = its main agent has no working child |
| `arbos_agentd_turns_running` | gauge | | main-agent turns running now |
| `arbos_agentd_sends_waiting` | gauge | | user messages waiting for a turn |
| `arbos_agentd_workers_running` | gauge | | worker (subagent) turns running now |
| `arbos_agentd_jobs` | gauge | `state` = `running` `queued` | background jobs |
| `arbos_agentd_rooms_open` | gauge | | rooms the box holds open |
| `arbos_agentd_rooms_connected` | gauge | | open rooms whose relay session is up; lower than `rooms_open` means the relay link is down or the room is busy |
| `arbos_agentd_turns_started_total` | counter | | main-agent turns started |
| `arbos_agentd_turns_finished_total` | counter | `result` = `done` `error` `cancelled` | main-agent turns finished |
| `arbos_agentd_turn_duration_seconds` | histogram | | main-agent turn duration; buckets 1, 2.5, 5, 10, 30, 60, 120, 300, 600, 1800 |
| `arbos_agentd_worker_crashes_total` | counter | `kind` = `main` `job` | `main`: the main agent's child died and was respawned; `job`: a job's worker failed and got a fresh one |
| `arbos_agentd_model_requests_total` | counter | `class` = `2xx`…`5xx` `unreachable` | model calls the box forwarded to the relay |
| `arbos_agentd_relay_reconnects_total` | counter | | room sessions dialled again after the first |

agentd's counters start at zero when the box boots.

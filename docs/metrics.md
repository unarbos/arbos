# Metrics

Prometheus metrics for the relay. The code is `crates/arbos-relay/src/metrics.rs`; a test there
fails when a family is missing from this page.

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
| `arbos_relay_notifications_total` | counter | `result` = `enqueued` `deduped` `delivered` `retried` `dropped_expired` `dropped_permanent` `dropped_no_token` | device notifications (APNs) |
| `arbos_relay_model_requests_total` | counter | `provider` = `openrouter` `anthropic` `openai` `other`, `class` = `2xx`…`5xx` `unreachable` | upstream model calls, by the provider the relay sent them to |
| `arbos_relay_model_response_seconds` | histogram | | time for the provider to answer with response headers (not the whole stream) |
| `arbos_relay_tokens_total` | counter | `kind` = `input` `output` `cache_read` `cache_write` | tokens the proxy metered, every caller included. The four kinds are disjoint (`input` is uncached input). Counted when a call ends, because the provider reports usage last; present at 0 from the start |
| `arbos_relay_spend_today_usd` | gauge | | relay-wide debits since 00:00 UTC, machine cost excluded; what the kill switch compares with the cap |
| `arbos_relay_spend_daily_cap_usd` | gauge | | global daily spend cap (`ARBOS_GLOBAL_DAILY_CAP_USD`) |
| `arbos_relay_kill_switch` | gauge | | 1 while the kill switch is on and every spend is refused. Read only: an automatic trip from an earlier UTC day reads 0 without the stored switch being touched |
| `arbos_relay_http_requests_total` | counter | `route`, `class` = `1xx`…`5xx` | HTTP requests. A websocket upgrade is `1xx` |
| `arbos_relay_http_request_seconds_total` | counter | `route`, `class` | seconds spent until the response headers; divide by the request rate for a mean. An upgrade or a stream counts only its start |
| `arbos_relay_db_writer_wait_seconds` | histogram | | how long a caller waits for the database writer lock, probed every 10 s |
| `process_resident_memory_bytes` | gauge | | resident memory (Linux) |
| `process_open_fds` | gauge | | open file descriptors (Linux) |
| `process_cpu_seconds_total` | counter | | user + system CPU time (Linux) |
| `process_start_time_seconds` | gauge | | start time, seconds since the epoch |

Every histogram uses the same buckets, in seconds: 0.001, 0.005, 0.025, 0.1, 0.25, 0.5, 1, 2.5, 5,
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
the call; nothing is read from or written to the database for them.

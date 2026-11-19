# Metrics

Prometheus metrics for the relay. The code is `crates/arbos-relay/src/metrics.rs`; a test there
fails when a family is missing from this page.

## Scraping

| what | value |
|---|---|
| address | `http://<ARBOS_RELAY_METRICS_LISTEN>/metrics`, e.g. `http://127.0.0.1:9108/metrics` |
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
code, version. No series carries a user, chat, room, project, token or address. `route` is the
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

Every histogram uses the same buckets, in seconds: 0.001, 0.005, 0.025, 0.1, 0.25, 0.5, 1, 2.5, 5,
10, 30, 60, 120, 300.

Counters and histograms start at zero when the relay starts. A series with labels appears with its
first event.

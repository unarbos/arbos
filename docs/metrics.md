# Metrics

Prometheus metrics for the relay and for each box's agentd. The code is `metrics.rs` in
`crates/arbos-relay/src` and in `crates/arbos-agentd/src`; a test in each fails when a family is
missing from this page.

## Scraping

| what | value |
|---|---|
| address | `http://<ARBOS_RELAY_METRICS_LISTEN>/metrics`, e.g. `http://127.0.0.1:9108/metrics` |
| auth | none; the listener is loopback-only |
| format | Prometheus text exposition 0.0.4 |

The endpoint is on a listener of its own, not on the relay's main port. `ARBOS_RELAY_METRICS_LISTEN`
must be a loopback socket address (`127.0.0.1:PORT`); the relay refuses to start on anything else,
and with the variable unset there is no endpoint at all. Caddy proxies only to the main listener, so
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

`<id>` is 12 hex characters derived from the box's owner, stable across restarts; it is not a
user id and no other label identifies a box. Use the listener's IP address in the discovery URL
(`127.0.0.1`, not `localhost`): the relay hands that address back as the scrape target. Only awake
boxes are listed, so a sleeping box is absent, not down. A listed box that answers `502` is
unreachable, or runs an image from before agentd had `/metrics`; `arbos_relay_box_scrapes` on the
relay says which, and how many.

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
| `arbos_relay_box_scrapes` | gauge | `result` = `ok` `old_image` `unreachable` `pending` | awake boxes by how their last agentd scrape through the relay ended. `old_image`: the box answered but has no `/metrics` yet, so it shows no agentd series until it is re-imaged. `pending`: awake, not scraped yet |
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
| `arbos_relay_kill_switch` | gauge | | 1 while the kill switch is on and every spend is refused |
| `arbos_relay_http_requests_total` | counter | `route`, `class` = `1xx`…`5xx` | HTTP requests. A websocket upgrade is `1xx` |
| `arbos_relay_http_request_seconds_total` | counter | `route`, `class` | seconds spent until the response headers; divide by the request rate for a mean. An upgrade or a stream counts only its start |
| `arbos_relay_db_writer_wait_seconds` | histogram | | how long a caller waits for the database writer lock, probed every 10 s |
| `process_resident_memory_bytes` | gauge | | resident memory (Linux) |
| `process_open_fds` | gauge | | open file descriptors (Linux) |
| `process_cpu_seconds_total` | counter | | user + system CPU time (Linux) |
| `process_start_time_seconds` | gauge | | start time, seconds since the epoch |

Every relay histogram uses the same buckets, in seconds: 0.001, 0.005, 0.025, 0.1, 0.25, 0.5, 1, 2.5, 5,
10, 30, 60, 120, 300.

Counters and histograms start at zero when the relay starts. A series with labels appears with its
first event.

## agentd

One set of series per box (label `box`, see above).

| name | type | labels | meaning |
|---|---|---|---|
| `arbos_agentd_build_info` | gauge | `version` | always 1 |
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

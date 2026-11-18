#!/bin/sh
# Local stand-in for the relay: serves /healthz, /metrics and /box-metrics on 127.0.0.1:7700 with
# the names from the metric contract (README), counters growing with the clock so rates and
# percentiles have something to show. It also serves stand-ins for what cAdvisor reports about the
# relay's systemd service on the real host, which a Mac has no equivalent of. Not used in production.
mkdir -p /www && cd /www && echo '{"ok":true}' > healthz
httpd -p 127.0.0.1:7700 -h /www
start=$(date +%s)
while :; do
  t=$(( $(date +%s) - 1700000000 ))
  wave=$(( t / 10 % 12 ))
  {
    cat <<M
process_resident_memory_bytes $(( 22000000000 + wave * 50000000 ))
process_open_fds $(( 900 + wave * 7 ))
process_start_time_seconds $start
process_cpu_seconds_total $(( t * 3 ))
arbos_relay_build_info{version="0.6.422"} 1
arbos_relay_ws_connections $(( 240 + wave * 5 ))
arbos_relay_box_connections 72
arbos_relay_http_requests_total{class="2xx"} $(( t * 40 ))
arbos_relay_http_requests_total{class="4xx"} $(( t * 2 ))
arbos_relay_http_requests_total{class="5xx"} $(( t / 20 ))
arbos_boxes{state="running"} $(( 70 + wave / 4 ))
arbos_boxes{state="starting"} $(( wave / 6 ))
arbos_boxes{state="asleep"} $(( 14 - wave / 4 ))
arbos_boxes{state="stopped"} 3
arbos_boxes{state="error"} 0
arbos_boxes_waitlisted 0
arbos_boxes_max 100
arbos_box_starts_total{result="ok"} $(( t / 30 ))
arbos_box_starts_total{result="error"} $(( t / 3000 ))
arbos_box_evictions_total $(( t / 6000 ))
arbos_model_requests_total{provider="openrouter",result="ok"} $(( t * 6 ))
arbos_model_requests_total{provider="openrouter",result="error"} $(( t / 9 ))
arbos_model_requests_total{provider="anthropic",result="ok"} $(( t * 2 ))
arbos_model_requests_total{provider="anthropic",result="error"} $(( t / 40 ))
arbos_model_request_duration_seconds_bucket{le="1"} $(( t * 2 ))
arbos_model_request_duration_seconds_bucket{le="5"} $(( t * 6 ))
arbos_model_request_duration_seconds_bucket{le="30"} $(( t * 8 - t / 10 ))
arbos_model_request_duration_seconds_bucket{le="+Inf"} $(( t * 8 ))
arbos_model_request_duration_seconds_count $(( t * 8 ))
arbos_model_request_duration_seconds_sum $(( t * 30 ))
container_memory_rss{id="/system.slice/arbos-relay.service"} $(( 22500000000 + wave * 50000000 ))
container_memory_working_set_bytes{id="/system.slice/arbos-relay.service"} $(( 23000000000 + wave * 50000000 ))
container_cpu_usage_seconds_total{id="/system.slice/arbos-relay.service"} $(( t * 3 ))
container_sockets{id="/system.slice/arbos-relay.service"} $(( 1100 + wave * 9 ))
container_start_time_seconds{id="/system.slice/arbos-relay.service"} $start
container_start_time_seconds{id="/system.slice/caddy.service"} $(( start - 86400 ))
M
  } > metrics.tmp && mv metrics.tmp metrics
  {
    for n in 1 2 3; do
      box=test_$n
      cat <<M
arbos_agent_turns_started_total{box="$box"} $(( t * n / 4 ))
arbos_agent_turns_finished_total{box="$box",result="ok"} $(( t * n / 4 - t * n / 90 - n ))
arbos_agent_turns_finished_total{box="$box",result="failed"} $(( t * n / 100 ))
arbos_agent_turns_finished_total{box="$box",result="cancelled"} $(( t * n / 900 ))
arbos_agent_turn_duration_seconds_bucket{box="$box",le="5"} $(( t * n / 20 ))
arbos_agent_turn_duration_seconds_bucket{box="$box",le="30"} $(( t * n / 7 ))
arbos_agent_turn_duration_seconds_bucket{box="$box",le="120"} $(( t * n / 5 ))
arbos_agent_turn_duration_seconds_bucket{box="$box",le="600"} $(( t * n * 6 / 25 ))
arbos_agent_turn_duration_seconds_bucket{box="$box",le="+Inf"} $(( t * n / 4 ))
arbos_agent_turn_duration_seconds_count{box="$box"} $(( t * n / 4 ))
arbos_agent_turn_duration_seconds_sum{box="$box"} $(( t * n * 15 ))
arbos_agent_turns_active{box="$box"} $(( n + wave / 5 ))
arbos_agent_queue_depth{box="$box"} $(( wave / (5 - n) ))
arbos_agent_workers{box="$box"} $(( n + 1 ))
arbos_agent_worker_crashes_total{box="$box"} $(( t * n / 5000 ))
M
    done
  } > box-metrics.tmp && mv box-metrics.tmp box-metrics
  sleep 5
done

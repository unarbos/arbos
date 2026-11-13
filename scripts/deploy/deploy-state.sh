#!/usr/bin/env bash
# Shared request receipts and effect admission; no configuration is read here.

deploy_lock_file() {
  local file="$1"
  [[ ! -L "$file" ]] || return 1
  if [[ ! -e "$file" ]]; then
    (umask 022; set -o noclobber; : > "$file") 2>/dev/null || [[ -f "$file" ]] || return 1
  fi
  [[ -f "$file" && ! -L "$file" ]]
}

deploy_queue_locked() {
  flock -x 8 || return 1
  "$@"
}

deploy_queue_transaction() {
  local queue="$1"; shift
  mkdir -p "$queue" || return 1
  deploy_lock_file "$queue/.queue.lock" || return 1
  deploy_queue_locked "$@" 8< "$queue/.queue.lock"
}

deploy_request_write_revision() {
  local kind="$1" value="$2" tmp
  tmp="$(mktemp "$QUEUE/.$kind.revision.XXXXXX")" || return 1
  if ! { printf '%s\n' "$value" > "$tmp" && chmod 0644 "$tmp" && mv -f "$tmp" "$QUEUE/.$kind.revision"; }; then
    rm -f "$tmp"
    return 1
  fi
}

deploy_request_ensure_revision() {
  local kind="$1" file="$QUEUE/.$1.revision"
  case "$kind" in relay|voice|agentd) ;; *) return 1 ;; esac
  [[ ! -L "$file" ]] || return 1
  if [[ ! -e "$file" ]]; then deploy_request_write_revision "$kind" 0 || return 1; fi
  [[ -f "$file" ]]
}

deploy_request_revision() {
  local file="$QUEUE/.$1.revision" value
  [[ -f "$file" && ! -L "$file" ]] || return 1
  (( $(wc -c < "$file") <= 19 )) || return 1
  value="$(cat "$file")" || return 1
  [[ "$value" =~ ^(0|[1-9][0-9]{0,17})$ ]] || return 1
  (( $(wc -c < "$file") == ${#value} + 1 )) || return 1
  printf '%s\n' "$value"
}

deploy_request_advance() {
  local kind="$1" revision
  deploy_request_ensure_revision "$kind" || return 1
  revision="$(deploy_request_revision "$kind")" || return 1
  (( revision < 999999999999999999 )) || return 1
  # Advance before mutation: a later write/unlink failure only refuses stale work.
  deploy_request_write_revision "$kind" "$((revision + 1))"
}

deploy_request_status() {
  local file="$1" value
  [[ -d "${file%/*}" && -x "${file%/*}" ]] || return 1
  if [[ ! -e "$file" && ! -L "$file" ]]; then echo -; return 0; fi
  [[ -f "$file" && ! -L "$file" && -r "$file" ]] || return 1
  (( $(wc -c < "$file") <= 41 )) || return 1
  value="$(cat "$file")" || return 1
  [[ "$value" =~ ^[0-9a-f]{7,40}$ ]] || return 1
  printf '%s\n' "$value"
}

deploy_request_installed() {
  case "$1" in
    relay) deploy_request_status /etc/arbos/relay.deployed-sha ;;
    voice) deploy_request_status /etc/arbos/voice.deployed-sha ;;
    agentd) deploy_request_status "${ARBOS_DEPLOY_STATE:-/var/lib/arbos-deploy}/agentd.deployed-sha" ;;
    *) return 1 ;;
  esac
}

deploy_request_default() {
  case "$1" in
    relay|voice) echo - ;;
    agentd) deploy_request_status "${ARBOS_DEPLOY_STATE:-/var/lib/arbos-deploy}/agentd.rolling.sha" ;;
    *) return 1 ;;
  esac
}

deploy_receipt_valid() {
  [[ "$1" =~ ^(relay|voice|agentd):(0|[1-9][0-9]{0,17}):(-|[0-9a-f]{7,40}):(-|[0-9a-f]{7,40}):(-|[0-9a-f]{7,40})$ ]]
}

deploy_publication_state() {
  if ! deploy_receipt_valid "$1" || ! deploy_receipt_valid "$2"; then echo Invalid; return 1; fi
  if [[ "$1" == "$2" ]]; then echo Publish; else echo Superseded; fi
}

deploy_request_observe_locked() {
  local kind="$1" revision queued=- installed current_default file="$QUEUE/$1"
  deploy_request_ensure_revision "$kind" || return 1
  revision="$(deploy_request_revision "$kind")" || return 1
  if [[ -e "$file" || -L "$file" ]]; then
    deploy_request_valid "$file" "$kind" || return 1
    queued="$(sed -n 's/^sha=//p' "$file")" || return 1
  fi
  installed="$(deploy_request_installed "$kind")" || return 1
  current_default="$(deploy_request_default "$kind")" || return 1
  local receipt="$kind:$revision:$queued:$installed:$current_default"
  deploy_receipt_valid "$receipt" || return 1
  printf '%s\n' "$receipt"
}

deploy_request_observe() {
  deploy_queue_transaction "$QUEUE" deploy_request_observe_locked "$1"
}

deploy_request_valid() {
  local file="$1" expected="$2" key value seen=0 bit kind=""
  [[ -f "$file" && ! -L "$file" ]] || return 1
  (( $(wc -c < "$file") <= 4096 )) || return 1
  while IFS='=' read -r key value || [[ -n "$key$value" ]]; do
    case "$key" in
      kind) bit=1; case "$value" in relay|voice|agentd) kind="$value" ;; *) return 1 ;; esac ;;
      sha) bit=2; [[ "$value" =~ ^[0-9a-f]{7,40}$ ]] || return 1 ;;
      requested_at) bit=4; [[ "$value" =~ ^(0|[1-9][0-9]{0,17})$ ]] || return 1 ;;
      first_requested_at) bit=8; [[ "$value" =~ ^(0|[1-9][0-9]{0,17})$ ]] || return 1 ;;
      requested_by) bit=16; [[ "$value" =~ ^[A-Za-z0-9_.-]{1,64}$ ]] || return 1 ;;
      *) return 1 ;;
    esac
    (( (seen & bit) == 0 )) || return 1
    seen=$((seen | bit))
  done < "$file"
  [[ "$kind" == "$expected" ]] && (( seen == 31 ))
}

deploy_request_snapshot_locked() {
  local queued="$1" snapshot="$2"
  deploy_request_valid "$queued" "${queued##*/}" || return 1
  cp "$queued" "$snapshot"
}

deploy_request_snapshot() {
  deploy_queue_transaction "$QUEUE" deploy_request_snapshot_locked "$@"
}

deploy_request_transition() {
  case "$1:$2" in
    Observed:Unchanged) echo Acknowledged ;;
    Observed:Changed) echo Superseded ;;
    Acknowledged:*|Superseded:*) echo "$1" ;;
    *) echo Invalid; return 1 ;;
  esac
}

deploy_request_ack_locked() {
  local queued="$1" snapshot="$2" state=Observed event=Changed
  cmp -s "$queued" "$snapshot" && event=Unchanged
  state="$(deploy_request_transition "$state" "$event")" || return 1
  if [[ "$state" == Acknowledged ]]; then
    deploy_request_advance "${queued##*/}" || return 1
    rm -f "$queued"
  fi
}

deploy_request_ack() {
  deploy_queue_transaction "$QUEUE" deploy_request_ack_locked "$@"
}

deploy_request_ack_commit_locked() {
  local kind="$1" sha="$2" queued="$QUEUE/$1" state=Observed event=Changed
  deploy_request_valid "$queued" "$kind" || return 0
  [[ "$(sed -n 's/^sha=//p' "$queued")" == "$sha" ]] && event=Unchanged
  state="$(deploy_request_transition "$state" "$event")" || return 1
  if [[ "$state" == Acknowledged ]]; then
    deploy_request_advance "$kind" || return 1
    rm -f "$queued"
  fi
}

deploy_request_ack_commit() {
  deploy_queue_transaction "$QUEUE" deploy_request_ack_commit_locked "$@"
}

deploy_lane_transition() {
  case "$1:$2" in
    Checking:Locked) echo Inspecting ;;
    Checking:Busy|Inspecting:Blocked) echo Deferred ;;
    Inspecting:Clear) echo Granted ;;
    Granted:Finished) echo Released ;;
    Deferred:*|Released:*) echo "$1" ;;
    *) echo Invalid; return 1 ;;
  esac
}

deploy_lane_marker_fresh() {
  local file="$1" at
  [[ -f "$file" ]] || return 1
  at="$(stat -c %Y "$file" 2>/dev/null)" || return 1
  [[ "$at" =~ ^[0-9]{1,18}$ ]] || return 1
  (( $(date +%s) - at < LANE_MARKER_MAX_S ))
}

# Exit75 means admission deferred; other failures retain their ordinary exit status.
# The subshell owns both the descriptor and EXIT cleanup, including caller cancellation.
deploy_lane_run() (
  local kind="$1" marker state=Checking rc; shift
  case "$kind" in
    relay) marker=relay.deploying ;;
    canary) marker=agentd.canary ;;
    www) marker=www.reloading ;;
    *) return 76 ;;
  esac
  mkdir -p "$STATE" || return 76
  deploy_lock_file "$STATE/.lane.lock" || return 76
  exec 7< "$STATE/.lane.lock" || return 76
  if flock -n 7; then
    state="$(deploy_lane_transition "$state" Locked)" || return 76
  else
    rc=$?
    (( rc == 1 )) || return 76
    state="$(deploy_lane_transition "$state" Busy)" || return 76
    return 75
  fi
  local other event=Clear
  for other in relay.deploying agentd.canary www.reloading; do
    [[ "$other" == "$marker" ]] && continue
    if deploy_lane_marker_fresh "$STATE/$other"; then event=Blocked; break; fi
  done
  state="$(deploy_lane_transition "$state" "$event")" || return 76
  [[ "$state" == Granted ]] || return 75
  trap 'rm -f "$STATE/$marker"' EXIT
  trap 'exit 143' TERM
  trap 'exit 130' INT
  printf 'kind=%s\npid=%s\nat=%s\n' "$kind" "$$" "$(date -u +%FT%TZ)" > "$STATE/$marker" || return 76
  if "$@"; then rc=0; else rc=$?; fi
  state="$(deploy_lane_transition "$state" Finished)" || return 76
  return "$rc"
)

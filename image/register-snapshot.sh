#!/usr/bin/env bash
# Register an image as a Daytona snapshot and wait until it is active (sandbox-image.md §6 step 6).
#
#   image/register-snapshot.sh <snapshot-name> <image-ref> [cpu] [memory-gb] [disk-gb]
#
# cpu/memory/disk default to 4/8/10 and must each be a positive integer; an empty value is refused
# (never silently registered). Memory is pinned so a box created from the snapshot does not fall back to
# Daytona's 1 GiB default. Daytona also caps memory per sandbox (8 GB in the arbos org — a larger value
# is rejected with HTTP 400), so memory-gb must not exceed that cap.
#
# Env: DAYTONA_API_KEY (required), DAYTONA_API_URL (default https://app.daytona.io/api).
# For a private GHCR image also set GHCR_USER + GHCR_READ_TOKEN: the script registers GHCR as a Daytona
# registry once (name "ghcr") so runners can pull.
#
# Without CI (repo not published), build on a Docker-capable host or a Daytona Docker-in-Docker sandbox,
# push to Daytona's transient registry, and call this script with that image ref:
#   creds=$(curl -s -H "Authorization: Bearer $DAYTONA_API_KEY" \
#     "$DAYTONA_API_URL/docker-registry/registry-push-access?snapshotName=<name>")   # username, secret, registryUrl, project
#   docker login cr.app.daytona.io -u <username> -p <secret>
#   docker tag arbos-agent:<v> cr.app.daytona.io/<project>/arbos-agent:<v> && docker push cr.app.daytona.io/<project>/arbos-agent:<v>
#   image/register-snapshot.sh arbos-agent-<v> cr.app.daytona.io/<project>/arbos-agent:<v> 4 8 10
set -euo pipefail

NAME=${1:?snapshot name}
IMAGE=${2:?image ref}
# `${N-default}` (not `${N:-default}`): an omitted resource takes the default, but an explicitly empty
# one stays empty and fails the check below — a caller that passes a blank memory must not silently get
# a snapshot. Daytona keys snapshot memory as `memory` (GB) on create and returns it as `mem`; when it
# is not pinned Daytona defaults the snapshot to 1 GiB, and every box created from the snapshot inherits
# that 1 GiB (boxes create from the snapshot and send no resources of their own). That is how the fleet
# rolled at 1 GiB. Pin an explicit, positive memory here, always.
CPU=${3-4}
MEM=${4-8}
DISK=${5-10}
for pair in "cpu:$CPU" "memory:$MEM" "disk:$DISK"; do
  k=${pair%%:*}; v=${pair#*:}
  [[ "$v" =~ ^[0-9]+$ ]] && (( v > 0 )) \
    || { echo "register-snapshot: $k must be a positive integer (got '${v:-<empty>}'); refusing to register $NAME with an unpinned resource" >&2; exit 2; }
done
API=${DAYTONA_API_URL:-https://app.daytona.io/api}
: "${DAYTONA_API_KEY:?DAYTONA_API_KEY is required}"
auth=(-H "Authorization: Bearer $DAYTONA_API_KEY" -H "content-type: application/json")

if [[ "$IMAGE" == ghcr.io/* && -n "${GHCR_READ_TOKEN:-}" ]]; then
  if ! curl -fsS "${auth[@]}" "$API/docker-registry" | jq -e '.[] | select(.name == "ghcr")' >/dev/null; then
    curl -fsS "${auth[@]}" -X POST "$API/docker-registry" \
      -d "$(jq -n --arg u "${GHCR_USER:-token}" --arg p "$GHCR_READ_TOKEN" '{name:"ghcr", url:"https://ghcr.io", username:$u, password:$p, project:""}')" >/dev/null
    echo "registered GHCR as a Daytona registry"
  fi
fi

existing=$(curl -fsS "${auth[@]}" "$API/snapshots/$NAME" 2>/dev/null || true)
# Re-register unless the existing snapshot matches on image AND resources. The resource match is what
# makes a drifted snapshot self-heal: a snapshot left at mem=1 (Daytona's default) for the right image
# used to count as "already active" and was skipped on every later roll, so the fleet never left 1 GiB.
# The response returns memory under `mem` (create takes it under `memory`).
if [ -n "$existing" ] \
  && [ "$(echo "$existing" | jq -r .state)" = "active" ] \
  && [ "$(echo "$existing" | jq -r .imageName)" = "$IMAGE" ] \
  && [ "$(echo "$existing" | jq -r '.cpu // empty')" = "$CPU" ] \
  && [ "$(echo "$existing" | jq -r '.mem // empty')" = "$MEM" ] \
  && [ "$(echo "$existing" | jq -r '.disk // empty')" = "$DISK" ]; then
  echo "snapshot $NAME already active for $IMAGE (cpu $CPU, mem ${MEM}G, disk ${DISK}G)"; exit 0
fi
if [ -n "$existing" ]; then
  id=$(echo "$existing" | jq -r .id)
  curl -fsS "${auth[@]}" -X DELETE "$API/snapshots/$id" >/dev/null && echo "removed stale snapshot $NAME ($id)"
fi

created=$(curl -fsS "${auth[@]}" -X POST "$API/snapshots" \
  -d "$(jq -n --arg n "$NAME" --arg i "$IMAGE" --argjson c "$CPU" --argjson m "$MEM" --argjson d "$DISK" '{name:$n, imageName:$i, cpu:$c, memory:$m, disk:$d}')")
id=$(echo "$created" | jq -r .id)
echo "snapshot $NAME = $id ($(echo "$created" | jq -r .state))"

for _ in $(seq 1 180); do
  s=$(curl -fsS "${auth[@]}" "$API/snapshots/$id")
  state=$(echo "$s" | jq -r .state)
  case "$state" in
    active) echo "snapshot $NAME active (size $(echo "$s" | jq -r .size) GB)"; exit 0 ;;
    error|build_failed) echo "snapshot $NAME failed: $(echo "$s" | jq -r .errorReason)"; exit 1 ;;
  esac
  sleep 10
done
echo "timed out waiting for snapshot $NAME"; exit 1

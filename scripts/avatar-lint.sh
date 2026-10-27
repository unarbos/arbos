#!/usr/bin/env bash
# Avatar lint (Jacob's rule, 2026-10-26, ARB-394): every agent avatar on the Mac and the iPhone is
# drawn through ONE resolver keyed by agent id + avatar version — no surface builds its own URL
# or draws the copy it happened to be handed. Source scan; runs in CI's Layout job and by hand:
#
#   ./scripts/avatar-lint.sh
#
# Mac (crates/arbos-mac/src/ui): the leaf/initials drawers `avatars::project(`, `avatars::agent(`
# and the raw `.image_url` of a Project / AgentMember may only appear in `ui/avatars.rs`; every
# other surface calls `project_in` / `project_with` / `project_image` / `member_picture` /
# `agent_identity` + `agent_picture`. iPhone (apps/ios/Arbos/Arbos): a `Project`'s `.imageURL`
# is read only by the resolver (`Model/AgentIdentity.swift`, `UI/Avatar.swift`, `Store/AppModel.swift`,
# `Sync/RelayAPI.swift`, `Model/Model.swift`); surfaces draw `AgentAvatar(project:)`, which
# resolves through `AppModel.newestAgentAvatar`, or ask `model.projectImageURL(_:)`.
# A line that must draw a group / server / person picture, or draws from an identity the
# resolver already produced, carries `// avatar-lint: allow (<why>)`.
set -euo pipefail
cd "$(dirname "$0")/.."

# `grep -n` hits minus those whose own line or the next line carries the allow marker (rustfmt
# moves a trailing comment onto the line after a call).
unmarked() {
  while IFS= read -r hit; do
    [[ -z "$hit" ]] && continue
    local file=${hit%%:*} rest=${hit#*:} line
    line=${rest%%:*}
    if sed -n "${line}p;$((line + 1))p" "$file" | grep -q 'avatar-lint: allow'; then
      continue
    fi
    echo "$hit"
  done
}

fail=0
report() {
  echo "avatar-lint: $1"
  echo "$2" | sed 's/^/  /'
  fail=1
}

# ---- Mac --------------------------------------------------------------------------------------
mac_draw=$(grep -rnE 'avatars::(project|agent)\(' crates/arbos-mac/src/ui --include='*.rs' \
  | grep -v '^crates/arbos-mac/src/ui/avatars.rs:' \
  | unmarked || true)
if [[ -n "$mac_draw" ]]; then
  report "Mac: avatars::project()/agent() drawn outside ui/avatars.rs — use project_in / project_with / member_picture / agent_picture (or mark the line avatar-lint: allow for a group picture)" "$mac_draw"
fi

mac_url=$(grep -rnE '\.image_url' crates/arbos-mac/src/ui --include='*.rs' \
  | grep -v '^crates/arbos-mac/src/ui/avatars.rs:' \
  | grep -vE '^crates/arbos-mac/src/ui/(groups|sidebar|avatar_picker|servers)\.rs:' \
  | grep -vE '^crates/arbos-mac/src/ui/profiles/(group|server|channel|me|user)\.rs:' \
  | unmarked || true)
if [[ -n "$mac_url" ]]; then
  report "Mac: a surface reads a project's / member's .image_url itself — ask avatars::project_image / sync::agent_avatar::newest" "$mac_url"
fi

# ---- iPhone -----------------------------------------------------------------------------------
ios_url=$(grep -rnE '\.imageURL\b' apps/ios/Arbos/Arbos --include='*.swift' \
  | grep -vE '^apps/ios/Arbos/Arbos/(Model/AgentIdentity|UI/Avatar|Store/AppModel|Sync/RelayAPI|Model/Model)\.swift:' \
  | unmarked || true)
if [[ -n "$ios_url" ]]; then
  report "iPhone: a surface reads a Project's .imageURL itself — draw AgentAvatar(project:) or ask model.projectImageURL(_:)" "$ios_url"
fi

ios_async=$(grep -rnE 'AsyncImage\(' apps/ios/Arbos/Arbos/UI/Avatar.swift apps/ios/Arbos/Arbos/UI/LeafAvatar.swift 2>/dev/null \
  | unmarked || true)
if [[ -n "$ios_async" ]]; then
  report "iPhone: avatar pictures load through AvatarImageCache (cache-first, retried), not AsyncImage" "$ios_async"
fi

if [[ "$fail" -ne 0 ]]; then
  echo "avatar-lint: FAIL — see docs/process-rules.md (one avatar resolver)"
  exit 1
fi
echo "avatar-lint: ok"

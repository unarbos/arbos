#!/usr/bin/env bash
# Glass lint ([feedback mac-glass-adaptive-2026-10-26], #2801): on Glass the desktop shows through
# the window, so an opaque theme token painted as a fill is a foreign material rather than a
# slightly wrong grey. Most of those tokens already have a Glass-aware accessor beside them in
# `ui/theme.rs` which returns the Matte token unchanged and a wash of the text colour on Glass.
# Source scan; runs in CI's Layout job and by hand:
#
#   ./scripts/glass-lint.sh                  # the gate
#   ./scripts/glass-lint.sh --write-baseline # after a sweep: record what is left
#
# The rule: a BARE opaque surface token may not reach a fill — whatever control flow surrounds it.
# #2801 shipped a `.bg(theme.surface_2)` inside an `if selected` leg of the sidebar's collapse, and
# the Rust drift guard (`highlight::tests::no_hover_or_pressed_fill_outside_the_token`) could not
# see it: that guard recognises `.hover(…)`, `.active(…)`, `group_hover(…)` and `.when(selected, …)`
# shapes, so a bare `.bg(…)` under an `if` was invisible to it. This guard keys on the token rather
# than on the control flow around it, so the shape that shipped fails here.
#
# "Bare" means the token lands in the fill unmodified. `Theme::with_alpha(theme.bg, 0.55)` and
# `alpha(theme.bg, t)` are deliberately see-through — a scrim, a fade — and are not flagged.
# Text inside double quotes is not code: a lint fixture that quotes `.bg(theme.surface_2)` paints
# nothing, so quoted spans are removed before the line is read.
#
# Two ways a site may stand:
#
#   * `// glass-lint: allow (<why>)` on the line or the line above it — a judged, deliberate
#     opaque fill, in the house marker style of scripts/avatar-lint.sh. The reason is free text
#     and may not be empty: a surface that is opaque by design (a photo viewer's matte, a call's
#     stage) says so in its own words.
#   * an entry in scripts/glass-lint-baseline.txt — a site that predates this guard and has not
#     been judged for Glass yet. The baseline is a debt list, not an approval. It may only
#     shrink: a site that leaves the tree, or is converted to its accessor, must leave the file
#     in the same commit, and no site may be added to it without saying so in review.
set -euo pipefail
cd "$(dirname "$0")/.."

TREE=crates/arbos-mac/src
BASELINE=scripts/glass-lint-baseline.txt
# The accessors live in theme.rs and legitimately return these fields.
SKIP='^crates/arbos-mac/src/ui/theme.rs:'

# The opaque surface tokens of `ui::theme::Theme` — the ones painted behind content, where the
# desktop would otherwise show. Longest first so the alternation cannot stop at a prefix.
# Each one's Glass-aware accessor, where it has one:
#   pane_card -> pane_card_fill()   sidebar -> chrome()
#   surface_1 -> card() / popup()   surface_2 -> control_fill()   field -> field_fill()
#   bubble    -> segment_on_fill()
# `bg` (the page colour) and `code_bg` have none, so a site painting either needs its own call.
TOK='bubble_assistant|surface_1|surface_2|pane_card|code_bg|sidebar|bubble|field|bg'

# Where a bare token counts as a fill. Each pattern is matched against the line with its
# whitespace removed, so the spacing rustfmt happens to choose cannot hide a token.
#   .bg(theme.X)                  a fill, direct
#   .bg(if c { theme.X } else …)  a fill chosen by an inline if / match arm
#   washed(el, theme.X, theme)    the filled control's base (ui::highlight)
#   segment(el, on, theme.X, …)   the on segment's base
#   over(theme.X, wash)           a wash composited onto an opaque base
#   let fill = theme.X;           bound first, painted later
#   cancel_bg: theme.X,           stored in a palette struct, painted later
PATTERNS=(
  "\.bg\(theme\.($TOK)[,)]"
  "\.bg\([^)]*\{theme\.($TOK)\}"
  "\bwashed\([^;]*,theme\.($TOK)[,)]"
  "\bsegment\([^;]*,theme\.($TOK)[,)]"
  "\bover\(theme\.($TOK)[,)]"
  "\bover\([^;]*,theme\.($TOK)[,)]"
  "\blet(mut)?[a-z_0-9]*=theme\.($TOK);"
  "[a-z_0-9]+:theme\.($TOK),"
)

# A reason that is there: `allow (` and then something. An empty `allow ()`, or a bare `allow`,
# waives nothing.
REASON='glass-lint: allow \([^) ]'

# `// glass-lint: allow (<why>)` on the hit's own line, or anywhere in the comment block directly
# above it — rustfmt moves a trailing comment onto the line after a call, and a reason worth
# reading often wraps over two lines.
marked() {
  local file=$1 line=$2 i text
  if sed -n "${line}p" "$file" | grep -qE "$REASON"; then
    return 0
  fi
  for ((i = line - 1; i > 0; i--)); do
    text=$(sed -n "${i}p" "$file")
    [[ "$text" =~ ^[[:space:]]*// ]] || return 1
    if printf '%s' "$text" | grep -qE "$REASON"; then
      return 0
    fi
  done
  return 1
}

# Every unmarked site in the tree, as `path<TAB>the line, trimmed`. Line numbers are deliberately
# left out: the baseline has to survive edits above a site, and only the count of identical lines
# in a file carries information a reviewer can check.
sites() {
  while IFS= read -r hit; do
    [[ -z "$hit" ]] && continue
    local file=${hit%%:*} rest=${hit#*:} line text flat
    line=${rest%%:*}
    text=${rest#*:}
    flat=$(printf '%s' "$text" | sed -E 's/"[^"]*"/""/g' | tr -d '[:space:]')
    local pattern found=0
    for pattern in "${PATTERNS[@]}"; do
      if printf '%s' "$flat" | grep -qE -- "$pattern"; then
        found=1
        break
      fi
    done
    [[ "$found" -eq 1 ]] || continue
    marked "$file" "$line" && continue
    printf '%s\t%s\n' "$file" "${text#"${text%%[![:space:]]*}"}"
  done < <(grep -rnE "theme\.($TOK)\b" "$TREE" --include='*.rs' | grep -vE "$SKIP" || true)
}

# `count<TAB>path<TAB>line`, sorted: the shape of the baseline file.
tally() {
  sites | awk -F'\t' '{n[$0]++} END {for (k in n) printf "%d\t%s\n", n[k], k}' | LC_ALL=C sort -t'	' -k2,2 -k3,3
}

header() {
  cat <<'EOF'
# Opaque theme fills that predate scripts/glass-lint.sh (#2801's defect class).
#
# Each line is `count<TAB>file<TAB>source line`: how many times that exact line appears in that
# file. Line numbers are left out so the file survives edits above a site. Regenerate with
# `./scripts/glass-lint.sh --write-baseline` and commit the shrinkage with the change that earned
# it; `git diff` on this file is the sweep's progress.
#
# This is a DEBT LIST, NOT AN APPROVAL. A site here has not been judged for Glass. A site that is
# deliberately opaque belongs at the paint site with `// glass-lint: allow (<why>)` instead, where
# the next reader will see the reason.
EOF
}

if [[ "${1:-}" == "--write-baseline" ]]; then
  {
    header
    tally
  } >"$BASELINE.new"
  mv "$BASELINE.new" "$BASELINE"
  echo "glass-lint: wrote $BASELINE ($(grep -cv '^#' "$BASELINE") entries)"
  exit 0
fi

# A marker whose reason is empty — `// glass-lint: allow ()` or a bare `// glass-lint: allow` —
# waives nothing. Catch it here rather than letting it read as a judged exception.
empty=$(grep -rn 'glass-lint: allow' "$TREE" --include='*.rs' \
  | grep -vE "$REASON" || true)

fail=0
if [[ -n "$empty" ]]; then
  echo "glass-lint: an allow marker with no reason waives nothing — write \`// glass-lint: allow (<why>)\`:"
  echo "$empty" | sed 's/^/  /'
  fail=1
fi

drift=$(diff -u <(grep -v '^#' "$BASELINE") <(tally) || true)
if [[ -n "$drift" ]]; then
  echo "glass-lint: the opaque fills in crates/arbos-mac/src no longer match $BASELINE."
  echo "  A \`+\` line is a new opaque fill where the desktop can show through. Paint it with the"
  echo "  token's Glass-aware accessor (ui/theme.rs: control_fill / field_fill / card / popup /"
  echo "  pane_card_fill / chrome / segment_on_fill), or say why this one must be"
  echo "  opaque with \`// glass-lint: allow (<why>)\` on the line or the line above it."
  echo "  A \`-\` line is a site that has gone or been converted: rerun with --write-baseline and"
  echo "  commit the smaller file with the change that earned it."
  echo "$drift" | sed 's/^/  /'
  fail=1
fi

if [[ "$fail" -ne 0 ]]; then
  echo "glass-lint: FAIL — see docs/process-rules.md (one Glass-aware fill)"
  exit 1
fi
allowed=$(grep -rc 'glass-lint: allow (' "$TREE" --include='*.rs' | awk -F: '{n += $2} END {print n + 0}')
echo "glass-lint: ok — $(grep -cv '^#' "$BASELINE") unjudged sites in the baseline, $allowed judged at the paint site"

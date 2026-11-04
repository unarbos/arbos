#!/usr/bin/env bash
# pbxproj identifier lint (AF-856, 2026-11-04): every object identifier in an Xcode
# project.pbxproj must be defined once. Runs in CI's Layout job (Linux, no macOS runner) and
# by hand:
#
#   ./scripts/pbxproj-id-lint.sh [project.pbxproj ...]   # no argument = every one in the tree
#   ./scripts/pbxproj-id-lint.sh --self-test             # prove the lint still works
#
# Xcode writes each object as a 24-hex identifier and never checks the file for uniqueness.
# A new file added by copying an existing one can inherit that file's PBXFileReference and
# PBXBuildFile identifiers; the parser then keeps the LAST definition of each identifier and
# the earlier object silently leaves the target. Its tests are neither compiled nor run, the
# suite still reports green, and the only signal is a "member of multiple groups" warning
# buried in xcodebuild's output. That is how AvatarBackSwipeUITests.swift dropped
# TranscriptSpacingUITests.swift out of ArbosUITests for three days.
#
# What this checks, and what it does not. This reads the PROJECT FILE and catches the *cause*:
# an identifier declared twice. It does not read the built bundle, so it cannot tell you which
# file lost its target membership as a result. A bundle-side count of the test classes that
# actually shipped catches the *effect*, and the two are complementary rather than redundant —
# do not drop the count because this exists. The argument for keeping both is that xcodebuild's
# own warning names only the file whose reference is duplicated, never the file that was
# dropped, and then adds "(but membership in targets will be unaffected)", which is precisely
# the damage that did occur. This lint names BOTH files on every duplicate, which is the part
# the build output cannot give you; only a count of what shipped sees the hole itself.
#
# Exit status, and the three outcomes are deliberately distinct:
#   0  every identifier is defined once          ("OK" line per file, on stdout)
#   1  an identifier is defined more than once   (the duplicates are listed, on stderr)
#   2  the lint could not decide                 (no file, unreadable file, or a file whose
#                                                 definition lines this lint cannot see)
# Exit 2 is never silent and never looks like exit 0: a check whose "cannot tell" reads the
# same as its "all clear" is worse than no check at all.
set -euo pipefail
cd "$(dirname "$0")/.."

self="$PWD/scripts/pbxproj-id-lint.sh"

# A definition line is two tabs, the 24-hex identifier, then the ` /* name */` comment. Every
# other mention of an identifier in the file is indented more deeply and is not a definition.
check_file() {
  local file=$1

  if [[ ! -f $file ]]; then
    echo "pbxproj-id-lint: CANNOT CHECK $file — it is not a file; nothing was verified." >&2
    return 2
  fi
  if [[ ! -r $file ]]; then
    echo "pbxproj-id-lint: CANNOT CHECK $file — it is not readable; nothing was verified." >&2
    return 2
  fi

  awk -v file="$file" '
    /^\t\t[0-9A-F]{24} \/\*/ {
      id = substr($0, 3, 24)
      definitions++
      count[id]++
      line = $0
      sub(/^\t+/, "", line)
      where[id] = where[id] sprintf("\n    line %d: %s", NR, line)
    }
    END {
      if (definitions == 0) {
        printf "pbxproj-id-lint: CANNOT CHECK %s — no object definition lines matched; the file format changed and nothing was verified.\n", file > "/dev/stderr"
        exit 2
      }
      duplicates = 0
      for (id in count) {
        if (count[id] > 1) {
          duplicates++
          printf "pbxproj-id-lint: %s defines %s %d times:%s\n", file, id, count[id], where[id] > "/dev/stderr"
        }
      }
      if (duplicates > 0) {
        printf "pbxproj-id-lint: FAIL %s — %d duplicated identifier(s) among %d definitions. Xcode keeps the last definition of each, so the earlier object silently leaves the target and its tests stop running. Give the newer object fresh 24-hex identifiers.\n", file, duplicates, definitions > "/dev/stderr"
        exit 1
      }
      printf "pbxproj-id-lint: OK %s — %d object definitions, every identifier unique.\n", file, definitions
    }
  ' "$file"
}

# Positive control. A guard that has never failed is not known to work, so CI runs this
# beside the real check: a clean file must pass, a duplicated identifier must fail, and an
# absent file must report "cannot decide" rather than success.
self_test() {
  local dir failures=0
  dir=$(mktemp -d)
  # shellcheck disable=SC2064  # $dir is wanted now, not when the trap fires.
  trap "rm -rf '$dir'" RETURN

  printf '\t\tAA00000000000000000000FF /* One.swift */ = {isa = PBXFileReference; };\n\t\tAA0000000000000000000100 /* Two.swift */ = {isa = PBXFileReference; };\n' > "$dir/clean.pbxproj"
  printf '\t\tAA00000000000000000000FF /* One.swift */ = {isa = PBXFileReference; };\n\t\tAA00000000000000000000FF /* Two.swift */ = {isa = PBXFileReference; };\n' > "$dir/duplicated.pbxproj"
  : > "$dir/empty.pbxproj"

  # The control nobody writes, and the one that decides whether the lint survives contact with a
  # real project: an identifier is *referenced* many times — in a fileRef, in a group's children,
  # in a build phase's files — and every one of those is legitimate. Only the declaration counts.
  # A lint that fires here gets switched off by the first person it inconveniences.
  {
    printf '\t\tAA00000000000000000000FF /* One.swift */ = {isa = PBXFileReference; };\n'
    printf '\t\tAA0000000000000000000100 /* One.swift in Sources */ = {isa = PBXBuildFile; fileRef = AA00000000000000000000FF /* One.swift */; };\n'
    printf '\t\tAA0000000000000000000101 /* Sources */ = {\n\t\t\tisa = PBXGroup;\n\t\t\tchildren = (\n\t\t\t\tAA00000000000000000000FF /* One.swift */,\n\t\t\t);\n\t\t};\n'
    printf '\t\tAA0000000000000000000102 /* Sources */ = {\n\t\t\tisa = PBXSourcesBuildPhase;\n\t\t\tfiles = (\n\t\t\t\tAA0000000000000000000100 /* One.swift in Sources */,\n\t\t\t);\n\t\t};\n'
  } > "$dir/referenced.pbxproj"

  # A multi-line object carries its `isa` on the *next* line, so a scan keyed on `isa = ` lines
  # cannot see these at all. Two PBXGroups sharing an identifier is a real collision and must fire.
  printf '\t\tAA0000000000000000000201 /* A */ = {\n\t\t\tisa = PBXGroup;\n\t\t};\n\t\tAA0000000000000000000201 /* B */ = {\n\t\t\tisa = PBXGroup;\n\t\t};\n' > "$dir/multiline-duplicated.pbxproj"

  # Xcode indents with tabs. If that ever stops being true the lint must say it cannot tell,
  # never that the file is clean — this fixture holds a duplicate the tab anchor cannot reach.
  printf '        AA00000000000000000000FF /* One.swift */ = {isa = PBXFileReference; };\n        AA00000000000000000000FF /* Two.swift */ = {isa = PBXFileReference; };\n' > "$dir/space-indented.pbxproj"

  expect() {
    local want=$1 label=$2 got=0
    shift 2
    "$@" > /dev/null 2>&1 || got=$?
    if [[ $got -eq $want ]]; then
      echo "pbxproj-id-lint self-test: ok — $label exits $got"
    else
      echo "pbxproj-id-lint self-test: BROKEN — $label exits $got, expected $want" >&2
      failures=$((failures + 1))
    fi
  }

  expect 0 "a clean file passes" "$self" "$dir/clean.pbxproj"
  expect 0 "an identifier referenced many times but declared once passes" "$self" "$dir/referenced.pbxproj"
  expect 1 "a duplicated identifier fails" "$self" "$dir/duplicated.pbxproj"
  expect 1 "a duplicated multi-line object fails" "$self" "$dir/multiline-duplicated.pbxproj"
  expect 2 "a space-indented file cannot be decided" "$self" "$dir/space-indented.pbxproj"
  expect 2 "a file with no definition lines cannot be decided" "$self" "$dir/empty.pbxproj"
  expect 2 "an absent file cannot be decided" "$self" "$dir/not-here.pbxproj"

  if [[ $failures -gt 0 ]]; then
    echo "pbxproj-id-lint self-test: $failures of 7 controls wrong; the lint cannot be trusted." >&2
    return 1
  fi
  echo "pbxproj-id-lint self-test: all 7 controls behaved."
}

if [[ ${1:-} == --self-test ]]; then
  self_test
  exit $?
fi

files=("$@")
if [[ ${#files[@]} -eq 0 ]]; then
  while IFS= read -r found; do
    files+=("$found")
  done < <(find . -name project.pbxproj -not -path './.git/*' | sort)
fi

if [[ ${#files[@]} -eq 0 ]]; then
  echo "pbxproj-id-lint: CANNOT CHECK — no project.pbxproj in this tree; nothing was verified." >&2
  exit 2
fi

status=0
for file in "${files[@]}"; do
  outcome=0
  check_file "$file" || outcome=$?
  # 1 (a duplicate is proven) outranks 2 (undecided): the proven defect is the useful verdict.
  [[ $outcome -eq 0 || $status -eq 1 ]] || status=$outcome
done

exit "$status"

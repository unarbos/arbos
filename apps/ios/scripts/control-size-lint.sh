#!/bin/bash
# iPhone control-family drift guard ([feedback iphone-header-composer-sizing-2026-10-26]).
# Same rules as ArbosTests/ControlTokenLintTests: outside UI/Theme.swift no literal disc size,
# no hand-built field/pill, no old fieldBorder rim — unless the line carries
# `// control-size: exception <Controls.Exception case>`. Exit 1 on a hit.
cd "$(dirname "$0")/../Arbos/Arbos" || exit 2
P='GlassCircle\(size:\s*[0-9]|\.background\((Capsule\(\)|Circle\(\)|RoundedRectangle\([^)]*\))\.fill\(theme\.field\)|strokeBorder\(theme\.fieldBorder'
hits=$(grep -nE "$P" UI/*.swift App/*.swift | grep -v '^UI/Theme.swift' | grep -v 'control-size: \(exception\|source\)')
if [ -n "$hits" ]; then
  echo "control-size: hand-sized controls outside UI/Theme.swift (use Controls / .controlField() / GlassCircle, or mark the line):"
  echo "$hits"; exit 1
fi
cases=$(grep -oE 'case [a-zA-Z]+' <(sed -n '/enum Exception: String/,/^    }/p' UI/Theme.swift) | awk '{print $2}')
bad=$(grep -ohE 'control-size: exception \w+' UI/*.swift App/*.swift | awk '{print $3}' | sort -u | grep -vxF -f <(echo "$cases"))
if [ -n "$bad" ]; then echo "control-size: unknown exception case(s): $bad"; exit 1; fi
echo "control-size: ok"

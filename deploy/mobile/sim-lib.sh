# Shared helpers for the iPhone loop's simulator scenarios.
#   . "$(dirname "$0")/sim-lib.sh"
#
# The one that matters is `pt`. A screenshot of the loop's simulator is
# 472x1024 pixels; `idb ui tap` works in the device's 393x852 point space.
# So a coordinate measured off a still and passed straight to a tap lands
# about one row low, and the run still looks healthy — stills, timings and
# recordings all arriving — while the taps are in the wrong place. Cycle 46
# opened two wrong projects that way before the header gave it away.
#
# Every hardcoded tap already in these scripts is in points and is correct.
# It is only reading a coordinate off a screenshot that needs converting,
# which is exactly what anyone does when writing a new scenario.

# The simulator's screenshot size and the device's point size. Both are the
# iPhone 15 Pro's; change them together if the loop's device changes.
SIM_SHOT_W=${SIM_SHOT_W:-472}
SIM_SHOT_H=${SIM_SHOT_H:-1024}
SIM_PT_W=${SIM_PT_W:-393}
SIM_PT_H=${SIM_PT_H:-852}

# pt <pixels-on-a-screenshot> -> points for idb, vertical scale.
pt() { python3 -c "print(round($1 * $SIM_PT_H / $SIM_SHOT_H))"; }

# ptx <pixels-on-a-screenshot> -> points, horizontal scale. Separate from
# `pt` because the two ratios are not identical and a tap near an edge can
# miss on the difference.
ptx() { python3 -c "print(round($1 * $SIM_PT_W / $SIM_SHOT_W))"; }

# tap_shot <x-px> <y-px> <udid> — tap where something is on the last still.
tap_shot() { idb ui tap "$(ptx "$1")" "$(pt "$2")" --udid "$3"; }

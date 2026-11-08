#!/usr/bin/env python3
"""Write environment variables into a built `.xctestrun` so the UI test runner sees them.

    xctestrun-env.py <xctestrun file> KEY=VALUE [KEY=VALUE ...]

`xcodebuild` is documented to turn a `TEST_RUNNER_<KEY>` build setting into `<KEY>` in the UI
test runner's environment. On the hosted Xcode 26.3 image it does not: every `ios-capture.yml`
run that relied on it skipped `ArbosUITests/CaptureRigUITests` with "no ARBOS_SHOT_DIR" and wrote
no PNG, while still reporting `** TEST EXECUTE SUCCEEDED **` (runs 42068636411, 42095484867 and
42254724518). The variables are written here instead, into the file the run actually reads.

Both `.xctestrun` layouts are handled: FormatVersion 2 nests the targets under
`TestConfigurations`, version 1 keys them by name at the top level beside
`__xctestrun_metadata__`.

A UI test target is one that names a host app to drive (`UITargetAppPath`). Writing into a plain
unit-test target would be harmless but silent, so the script insists on finding at least one UI
test target and exits non-zero otherwise — a capture that cannot see its variables must fail at
this step, not twenty minutes later as an empty directory.
"""

import plistlib
import sys
from pathlib import Path


def targets(doc: dict) -> list[dict]:
    configs = doc.get("TestConfigurations")
    if configs is not None:
        return [t for c in configs for t in c.get("TestTargets", [])]
    return [v for k, v in doc.items() if k != "__xctestrun_metadata__" and isinstance(v, dict)]


def main(argv: list[str]) -> int:
    if len(argv) < 3:
        print(__doc__, file=sys.stderr)
        return 2
    path = Path(argv[1])
    env = {}
    for pair in argv[2:]:
        key, sep, value = pair.partition("=")
        if not sep or not key:
            print(f"not a KEY=VALUE pair: {pair!r}", file=sys.stderr)
            return 2
        env[key] = value

    doc = plistlib.loads(path.read_bytes())
    all_targets = targets(doc)
    written = []
    for target in all_targets:
        if not target.get("UITargetAppPath"):
            continue
        target.setdefault("EnvironmentVariables", {}).update(env)
        written.append(target.get("BlueprintName", "?"))

    if not written:
        names = [t.get("BlueprintName", "?") for t in all_targets]
        print(f"no UI test target in {path}; targets were {names}", file=sys.stderr)
        return 1

    path.write_bytes(plistlib.dumps(doc))
    print(f"{path.name}: wrote {sorted(env)} into {', '.join(written)}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

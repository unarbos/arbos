#!/usr/bin/env python3
"""Write environment variables into a built `.xctestrun` so the UI test runner sees them.

    xctestrun-env.py [--all-targets] [--verify] <xctestrun file> KEY=VALUE ...

`xcodebuild` is documented to turn a `TEST_RUNNER_<KEY>` build setting into `<KEY>` in the UI
test runner's environment. On the hosted Xcode 26.3 image it does not: every `ios-capture.yml`
run that relied on it skipped `ArbosUITests/CaptureRigUITests` with "no ARBOS_SHOT_DIR" and wrote
no PNG, while still reporting `** TEST EXECUTE SUCCEEDED **` (runs 42068636411, 42095484867 and
42254724518). The variables are written here instead, into the file the run actually reads.

Both `.xctestrun` layouts are handled: FormatVersion 2 nests the targets under
`TestConfigurations`, version 1 keys them by name at the top level beside
`__xctestrun_metadata__`.

A UI test target names an app to drive (`UITargetAppPath`). By default only its runner receives
the variables, preserving the capture path. `--all-targets` also covers unit-test hosts and UI
target apps. `--verify` reads back the expected values without changing the plan. Unsupported
layouts or missing targets/values fail before xcodebuild can launch a test host.
"""

import plistlib
import re
import sys
from pathlib import Path
from xml.parsers.expat import ExpatError


def targets(doc: dict) -> list[dict]:
    if not isinstance(doc, dict):
        raise ValueError("Unsupported test plan")
    metadata = doc.get("__xctestrun_metadata__")
    version = metadata.get("FormatVersion") if isinstance(metadata, dict) else None
    if type(version) is not int or version not in (1, 2):
        raise ValueError("Unsupported test plan version")
    if version == 2:
        configs = doc.get("TestConfigurations")
        if not isinstance(configs, list) or not configs:
            raise ValueError("Missing test configurations")
        found = []
        for config in configs:
            if not isinstance(config, dict) or not isinstance(config.get("TestTargets"), list):
                raise ValueError("Invalid test configuration")
            found.extend(config["TestTargets"])
    else:
        if "TestConfigurations" in doc:
            raise ValueError("Conflicting test plan layouts")
        found = [v for k, v in doc.items() if k != "__xctestrun_metadata__" and isinstance(v, dict)]
    for target in found:
        if not isinstance(target, dict) or any(not isinstance(target.get(key), str) or not target[key]
                                              for key in ("BlueprintName", "TestBundlePath", "TestHostPath")):
            raise ValueError("Invalid test target")
        if "UITargetAppPath" in target and (not isinstance(target["UITargetAppPath"], str) or not target["UITargetAppPath"]):
            raise ValueError("Invalid UI target")
    return found


def main(argv: list[str]) -> int:
    arguments = list(argv[1:])
    options = set()
    while arguments and arguments[0].startswith("--"):
        option = arguments.pop(0)
        if option not in {"--all-targets", "--verify"} or option in options:
            print("unsupported option", file=sys.stderr)
            return 2
        options.add(option)
    if len(arguments) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    path = Path(arguments[0])
    env = {}
    for pair in arguments[1:]:
        key, sep, value = pair.partition("=")
        if not sep or re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", key, flags=re.ASCII) is None or "\0" in value:
            print("invalid environment assignment", file=sys.stderr)
            return 2
        env[key] = value
    try:
        doc = plistlib.loads(path.read_bytes())
        selected = [target for target in targets(doc) if "--all-targets" in options or target.get("UITargetAppPath")]
        if not selected:
            raise ValueError("No selected test targets")
        for target in selected:
            fields = ["EnvironmentVariables"]
            if "--all-targets" in options and target.get("UITargetAppPath"):
                fields.append("UITargetAppEnvironmentVariables")
            for field in fields:
                existing = target.get(field, {})
                if not isinstance(existing, dict) or any(
                    not isinstance(k, str) or re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", k, flags=re.ASCII) is None
                    or not isinstance(v, str) or "\0" in v for k, v in existing.items()
                ):
                    raise ValueError("Invalid target environment")
                if "--verify" in options:
                    if any(existing.get(key) != value for key, value in env.items()):
                        raise ValueError("Missing expected target environment")
                else:
                    target[field] = {**existing, **env}
        if "--verify" not in options:
            path.write_bytes(plistlib.dumps(doc))
    except (OSError, ValueError, TypeError, RecursionError, plistlib.InvalidFileException, ExpatError) as error:
        # Neither malformed assignments nor plist parser errors may echo environment values.
        print(f"test plan validation failed ({type(error).__name__})", file=sys.stderr)
        return 1
    action = "verified" if "--verify" in options else "wrote"
    print(f"{path.name}: {action} {sorted(env)} for {len(selected)} test target(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

"""No Xcode/app launch: exercise the real plan helper and mocked Fastlane test lane."""
from copy import deepcopy
import json
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
HELPER = HERE / "xctestrun-env.py"
FASTFILE = ROOT / "apps/ios/Arbos/fastlane/Fastfile"
ENDPOINT = "http://127.0.0.1:1"


def target(name="ArbosTests", ui=False):
    value = {"BlueprintName": name, "TestBundlePath": f"__TESTROOT__/{name}.xctest",
             "TestHostPath": "__TESTROOT__/Arbos.app/Arbos",
             "EnvironmentVariables": {"KEEP": "unchanged"},
             "CommandLineArguments": ["preserved"], "Custom": {"data": b"bytes"}}
    if ui:
        value.update(UITargetAppPath="__TESTROOT__/Arbos.app",
                     UITargetAppEnvironmentVariables={"KEEP_APP": "unchanged"})
    return value


def plan(version=2, entries=None):
    entries = entries if entries is not None else [target(), target("ArbosUITests", ui=True)]
    doc = {"__xctestrun_metadata__": {"FormatVersion": version, "Preserved": "metadata"}}
    if version == 1:
        doc.update({entry["BlueprintName"]: entry for entry in entries})
    else:
        doc.update(TestConfigurations=[{"Name": "Default", "IsEnabled": True, "TestTargets": entries}],
                   TestPlan={"Name": "Arbos", "IsDefault": True}, CodeCoverageBuildableInfos=[])
    return doc


def entries(doc):
    return (doc["TestConfigurations"][0]["TestTargets"] if "TestConfigurations" in doc
            else [v for k, v in doc.items() if k != "__xctestrun_metadata__"])


class PlanTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="arbos-test-plan-")
        self.addCleanup(self.temporary.cleanup)
        self.path = Path(self.temporary.name) / "plan with spaces.xctestrun"

    def write(self, doc):
        self.path.write_bytes(plistlib.dumps(doc))

    def run_helper(self, *options, pair=f"ARBOS_RELAY_URL={ENDPOINT}"):
        return subprocess.run([sys.executable, str(HELPER), *options, str(self.path), pair],
                              text=True, capture_output=True, timeout=5)

    def test_default_changes_only_ui_runner_in_v1_and_v2(self):
        for version in (1, 2):
            with self.subTest(version=version):
                original = plan(version)
                self.write(original)
                result = self.run_helper()
                self.assertEqual(result.returncode, 0, result.stderr)
                changed = plistlib.loads(self.path.read_bytes())
                expected = deepcopy(original)
                entries(expected)[1]["EnvironmentVariables"]["ARBOS_RELAY_URL"] = ENDPOINT
                self.assertEqual(changed, expected)
                self.assertEqual(self.run_helper("--verify").returncode, 0)

    def test_all_targets_sets_host_and_ui_app_without_changing_other_values(self):
        for version in (1, 2):
            with self.subTest(version=version):
                original = plan(version)
                self.write(original)
                result = self.run_helper("--all-targets")
                self.assertEqual(result.returncode, 0, result.stderr)
                changed = plistlib.loads(self.path.read_bytes())
                expected = deepcopy(original)
                for item in entries(expected):
                    item["EnvironmentVariables"]["ARBOS_RELAY_URL"] = ENDPOINT
                    if "UITargetAppPath" in item:
                        item["UITargetAppEnvironmentVariables"]["ARBOS_RELAY_URL"] = ENDPOINT
                self.assertEqual(changed, expected)
                before = self.path.read_bytes()
                self.assertEqual(self.run_helper("--all-targets", "--verify").returncode, 0)
                self.assertEqual(self.path.read_bytes(), before)

    def test_unit_only_requires_explicit_all_targets(self):
        self.write(plan(entries=[target()]))
        before = self.path.read_bytes()
        self.assertNotEqual(self.run_helper().returncode, 0)
        self.assertEqual(self.path.read_bytes(), before)
        self.assertEqual(self.run_helper("--all-targets").returncode, 0)

    def test_verification_rejects_removed_host_or_ui_app_value(self):
        for location in ("unit", "ui-runner", "ui-app"):
            with self.subTest(location=location):
                self.write(plan())
                self.assertEqual(self.run_helper("--all-targets").returncode, 0)
                doc = plistlib.loads(self.path.read_bytes())
                field = "UITargetAppEnvironmentVariables" if location == "ui-app" else "EnvironmentVariables"
                entries(doc)[0 if location == "unit" else 1][field].pop("ARBOS_RELAY_URL")
                self.write(doc)
                before = self.path.read_bytes()
                self.assertNotEqual(self.run_helper("--all-targets", "--verify").returncode, 0)
                self.assertEqual(self.path.read_bytes(), before)

    def test_missing_empty_unknown_or_malformed_plan_fails_without_writing(self):
        invalid = [[], {}, {"__xctestrun_metadata__": {"FormatVersion": 3}},
                   {"__xctestrun_metadata__": {"FormatVersion": True}},
                   plan(1, []), plan(entries=[]), {**plan(1), "TestConfigurations": []},
                   {**plan(), "TestConfigurations": {"TestTargets": []}},
                   {**plan(), "TestConfigurations": [{"TestTargets": ["not a target"]}]}]
        broken = plan()
        del entries(broken)[0]["TestHostPath"]
        invalid.append(broken)
        for doc in invalid:
            with self.subTest(doc=doc):
                self.write(doc)
                before = self.path.read_bytes()
                self.assertNotEqual(self.run_helper("--all-targets").returncode, 0)
                self.assertEqual(self.path.read_bytes(), before)
        self.path.write_text("not a plist")
        self.assertNotEqual(self.run_helper("--all-targets").returncode, 0)
        self.assertEqual(self.path.read_text(), "not a plist")
        self.path.unlink()
        self.assertNotEqual(self.run_helper("--all-targets").returncode, 0)

    def test_bad_environment_keys_never_echo_values_or_mutate_plan(self):
        self.write(plan())
        before = self.path.read_bytes()
        for key in ("", "1BAD", "BAD-KEY", "BAD KEY", "BAD\nKEY", "ÜNICODE"):
            with self.subTest(key=key):
                result = self.run_helper("--all-targets", pair=f"{key}=synthetic-private-value")
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn("synthetic-private-value", result.stdout + result.stderr)
                self.assertEqual(self.path.read_bytes(), before)
        result = self.run_helper("--unknown=synthetic-private-value")
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("synthetic-private-value", result.stdout + result.stderr)

    def test_existing_environment_must_be_a_string_dictionary(self):
        for value in ([], "not a dictionary", {"BAD": 7}, {"BAD-KEY": "synthetic-private-value"}, {"BAD": "nul\0value"}):
            with self.subTest(value=value):
                doc = plan()
                entries(doc)[0]["EnvironmentVariables"] = value
                self.path.write_bytes(plistlib.dumps(doc, fmt=plistlib.FMT_BINARY))
                before = self.path.read_bytes()
                self.assertNotEqual(self.run_helper("--all-targets").returncode, 0)
                self.assertEqual(self.path.read_bytes(), before)

    def test_capture_values_can_contain_equals_and_newlines_without_value_output(self):
        self.write(plan())
        value = "shot step=one\nshot step=two"
        result = self.run_helper(pair=f"ARBOS_SHOT_SCRIPT={value}")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("shot step", result.stdout + result.stderr)
        self.assertEqual(entries(plistlib.loads(self.path.read_bytes()))[1]["EnvironmentVariables"]["ARBOS_SHOT_SCRIPT"], value)


RUBY_HARNESS = r'''
require 'json'
require 'fileutils'
module UI
  def self.user_error!(message); raise message; end
end
$lanes = {}
$calls = []
$tamper = ARGV[1]
def default_platform(*); end
def desc(*); end
def platform(*); yield; end
def lane(name, &block); $lanes[name] = block; end
load ARGV[0]
def require_xcframework!; end
def xcodebuild_logged(name, command)
  $calls << [name, command]
end
def sh(command)
  $calls << ['helper', command]
  raise 'helper failed; tests were not launched' unless system(command, out: File::NULL, err: File::NULL)
  if $tamper && !command.include?('--verify')
    code = "import plistlib,sys; from pathlib import Path; p=Path(sys.argv[1]); d=plistlib.loads(p.read_bytes()); d['TestConfigurations'][0]['TestTargets'][0]['EnvironmentVariables'].pop('ARBOS_RELAY_URL'); p.write_bytes(plistlib.dumps(d))"
    raise 'fixture tamper failed' unless system('python3', '-c', code, $tamper)
  end
end
begin
  $lanes[:test].call
  puts JSON.generate({calls: $calls, ok: true})
rescue => error
  puts JSON.generate({calls: $calls, ok: false, error: error.message})
  exit 1
end
'''


class LaneTests(unittest.TestCase):
    def setUp(self):
        if shutil.which("ruby") is None:
            self.skipTest("Ruby is required to exercise the actual Fastlane lane DSL")
        self.temporary = tempfile.TemporaryDirectory(prefix="arbos-mocked-lane-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name) / "project with spaces"
        self.fastfile = self.root / "apps/ios/Arbos/fastlane/Fastfile"
        self.fastfile.parent.mkdir(parents=True)
        shutil.copyfile(FASTFILE, self.fastfile)
        self.helper = self.root / "scripts/ci/xctestrun-env.py"
        self.helper.parent.mkdir(parents=True)
        shutil.copyfile(HELPER, self.helper)
        self.products = self.root / "apps/ios/Arbos/build/dd/Build/Products"
        self.products.mkdir(parents=True)
        self.path = self.products / "Arbos fixture.xctestrun"

    def run_lane(self, tamper=False):
        command = ["ruby", "-e", RUBY_HARNESS, str(self.fastfile)]
        if tamper:
            command.append(str(self.path))
        run = subprocess.run(command,
                             text=True, capture_output=True, timeout=10,
                             env={"PATH": str(Path(sys.executable).parent) + ":/usr/bin:/bin"})
        return run, json.loads(run.stdout)

    def test_actual_lane_builds_patches_verifies_then_launches_exact_plan(self):
        self.path.write_bytes(plistlib.dumps(plan()))
        run, result = self.run_lane()
        self.assertEqual(run.returncode, 0, run.stderr + run.stdout)
        self.assertEqual([name for name, _ in result["calls"]], ["test-build", "helper", "helper", "test"])
        self.assertIn("build-for-testing", result["calls"][0][1])
        self.assertIn("--verify", result["calls"][2][1])
        self.assertIn("test-without-building -xctestrun", result["calls"][3][1])
        self.assertNotIn("-only-testing", result["calls"][3][1])
        for item in entries(plistlib.loads(self.path.read_bytes())):
            self.assertEqual(item["EnvironmentVariables"]["ARBOS_RELAY_URL"], ENDPOINT)

    def test_missing_or_multiple_plans_fail_before_launch(self):
        for count in (0, 2):
            with self.subTest(count=count):
                for path in self.products.glob("*.xctestrun"):
                    path.unlink()
                for number in range(count):
                    (self.products / f"plan{number}.xctestrun").write_bytes(plistlib.dumps(plan()))
                run, result = self.run_lane()
                self.assertNotEqual(run.returncode, 0)
                self.assertEqual([name for name, _ in result["calls"]], ["test-build"])

    def test_readback_mismatch_stops_actual_lane_before_test_launch(self):
        self.path.write_bytes(plistlib.dumps(plan()))
        run, result = self.run_lane(tamper=True)
        self.assertNotEqual(run.returncode, 0)
        self.assertEqual([name for name, _ in result["calls"]], ["test-build", "helper", "helper"])
        self.assertNotIn("ARBOS_RELAY_URL", entries(plistlib.loads(self.path.read_bytes()))[0]["EnvironmentVariables"])

    def test_unsupported_plan_or_missing_helper_never_launches_tests(self):
        self.path.write_bytes(plistlib.dumps({"unsupported": True}))
        run, result = self.run_lane()
        self.assertNotEqual(run.returncode, 0)
        self.assertEqual([name for name, _ in result["calls"]], ["test-build", "helper"])
        self.path.write_bytes(plistlib.dumps(plan()))
        self.helper.unlink()
        run, result = self.run_lane()
        self.assertNotEqual(run.returncode, 0)
        self.assertNotIn("test", [name for name, _ in result["calls"]])


if __name__ == "__main__":
    unittest.main()

"""Actual private shell controllers; fresh files/fake observations, never shared locks."""
from pathlib import Path
import json
import os
import shutil
import subprocess
import tempfile
import time
import unittest

HERE = Path(__file__).resolve().parent
SOURCE = HERE.parent
SANDBOX_PROFILE = '(version 1) (allow default) (deny network*) (deny file-read* file-write* (subpath "/Users/Shared")) (deny process-exec (literal "/usr/bin/xcrun") (literal "/usr/bin/sudo") (literal "/usr/bin/open") (literal "/usr/bin/osascript"))'
OWNER = "900000001"
OTHER = "900000002"
FAKE = r'''#!/usr/bin/env python3
from pathlib import Path
import json,os,subprocess,sys,time
root=Path(os.environ['FIXTURE_ROOT']); tool=Path(sys.argv[0]).name; args=sys.argv[1:]
with (root/'calls.jsonl').open('a') as log:log.write(json.dumps({'tool':tool,'args':args})+'\n')
if tool=='flock':
 import fcntl
 try:fcntl.flock(int(args[-1]),fcntl.LOCK_EX|(fcntl.LOCK_NB if '-n' in args else 0))
 except BlockingIOError:sys.exit(1)
 sys.exit(0)
if tool=='ps':
 pid=args[args.index('-p')+1] if '-p' in args else ''
 if '-o' in args:
  field=args[args.index('-o')+1]
  print(('1' if pid==os.environ['FIXTURE_OWNER'] else os.environ['FIXTURE_OWNER']) if field=='ppid=' else '')
  sys.exit(0)
 if pid==os.environ.get('BLOCK_RETIRE_PID'):
  for slot in (root/'slots').glob('*-rig-holder.slot'):(slot/'retired').mkdir(exist_ok=True)
 count=root/('ps-'+pid); n=int(count.read_text())+1 if count.exists() else 1; count.write_text(str(n))
 dead=pid in os.environ.get('FIXTURE_DEAD','').split()
 if (root/'dead-pids').exists():dead=dead or pid in (root/'dead-pids').read_text().split()
 if pid==os.environ['FIXTURE_OWNER'] and os.environ.get('DIE_AFTER'):dead=n>int(os.environ['DIE_AFTER'])
 sys.exit(1 if dead else 0)
if tool=='date':
 print('1700000000' if any(a=='%s' or a=='+%s' for a in args) else '2023-11-14T22:13:20Z');sys.exit(0)
if tool=='sleep':
 (root/'sleeping').touch();deadline=time.monotonic()+4
 while not (root/'release-sleep').exists():
  if time.monotonic()>deadline:raise RuntimeError('Sleep fixture deadline')
  time.sleep(.01)
 sys.exit(0)
if tool=='sysctl':print('{ 0.0 0.0 0.0 }');sys.exit(0)
if tool=='logger':sys.exit(0)
if tool=='sudo':sys.exit(77)
if tool=='xcrun' and os.environ.get('FIXTURE_SIM_STATE'):
 print('Synthetic (AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE) ('+os.environ['FIXTURE_SIM_STATE']+')');sys.exit(0)
if tool in {'xcrun','fake-real'}:
 print('fake-xcrun');sys.exit(0)
if tool=='fake-bind':sys.exit(0)
if tool=='rm':
 if os.environ.get('FAIL_REMOVE') and any(a.endswith('.slot') for a in args):sys.exit(1)
 for arg in args:
  if arg.startswith('/') and not Path(arg).is_relative_to(root):raise RuntimeError('Outside fixture path')
 sys.exit(subprocess.run(['/bin/rm',*args]).returncode)
if tool in {'mkdir','mv'}:
 for arg in args:
  if arg.startswith('/') and not Path(arg).is_relative_to(root):raise RuntimeError('Outside fixture path')
 if os.environ.get('PAUSE_CLAIM') and any('rig-first' in a for a in args):
  (root/'claim-ready').touch()
  deadline=time.monotonic()+4
  while not (root/'release-claim').exists():
   if time.monotonic()>deadline:raise RuntimeError('Fixture claim barrier deadline')
   time.sleep(.01)
 if tool=='mv' and os.environ.get('PAUSE_TAKE'):
  (root/('ready-'+str(os.getpid()))).touch()
  # The test releases both candidates, or releases the one holding the mutex.
  deadline=time.monotonic()+4
  while not (root/'release-take').exists():
   if time.monotonic()>deadline:raise RuntimeError('Fixture barrier deadline')
   time.sleep(.01)
 result=subprocess.run(['/bin/'+tool,*args]);
 if result.returncode:sys.exit(result.returncode)
 if tool=='mkdir' and os.environ.get('INJECT_RECLAIM') and args[-1].endswith('.slot'):
  env=dict(os.environ);env.pop('INJECT_RECLAIM');subprocess.run(['/bin/bash',str(root/'arbos-heavy-slot.sh'),'reclaim'],env=env,check=True,stdout=subprocess.DEVNULL)
 sys.exit(0)
raise RuntimeError('Unexpected fake command')
'''


class Slots(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="arbos-slot-fixture-", dir="/tmp")
        self.root = Path(self.temp.name)
        self.addCleanup(self.temp.cleanup)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        helper = self.bin / "fake.py"
        helper.write_text(FAKE)
        helper.chmod(0o755)
        for tool in ["ps", "date", "sysctl", "logger", "sudo", "xcrun", "mkdir", "mv", "fake-real", "fake-bind", "sleep", "rm"]:
            (self.bin / tool).symlink_to(helper)
        self.slots = self.root / "slots"
        self.slots.mkdir()
        for name in ["arbos-heavy-slot.sh", "gui-lock.sh", "xcrun-slot-guard.sh"]:
            source = SOURCE / name
            text = source.read_text()
            if name == "arbos-heavy-slot.sh":
                expected = 'SLOTS_DIR="${ARBOS_HEAVY_SLOTS_DIR:-/Users/Shared/arbos-run/heavy-slots}"'
                self.assertEqual(text.count(expected), 1, "Allocator fixture adapter is stale")
                text = text.replace(expected, 'SLOTS_DIR="${ARBOS_HEAVY_SLOTS_DIR:-' + str(self.slots) + '}"')
            if name == "gui-lock.sh":
                expected = 'LOCK=${GUI_LOCK_FILE:-/Users/Shared/arbos-run/gui.lock}'
                self.assertEqual(text.count(expected), 1, "GUI fixture adapter is stale")
                text = text.replace(expected, 'LOCK=${GUI_LOCK_FILE:-' + str(self.root / "gui.lock") + '}')
            if source.name == "xcrun-slot-guard.sh":
                for expected in ["REAL=/usr/bin/xcrun", "SLOTS=/Users/Shared/arbos-run/heavy-slots", "/usr/local/bin/arbos-heavy-slot.sh bind"]:
                    self.assertEqual(text.count(expected), 1, "Boot fixture adapter is stale")
                text = text.replace("REAL=/usr/bin/xcrun", f"REAL={self.bin / 'fake-real'}")
                text = text.replace("SLOTS=/Users/Shared/arbos-run/heavy-slots", f"SLOTS={self.slots}")
                text = text.replace("/usr/local/bin/arbos-heavy-slot.sh bind", f"{self.bin / 'fake-bind'} bind")
            (self.root / source.name).write_text(text)
        self.env = {
            "PATH": f"{self.bin}:{Path(shutil.which('python3')).parent}:/usr/bin:/bin",
            "HOME": str(self.root / "home"), "FIXTURE_ROOT": str(self.root),
            "FIXTURE_OWNER": OWNER, "ARBOS_HEAVY_SLOTS_DIR": str(self.slots),
            "ARBOS_HEAVY_MAX_SLOTS": "1", "ARBOS_HEAVY_REFUSE_STOPS_OWNER": "0",
            "GUI_LOCK_FILE": str(self.root / "gui.lock"), "GUI_LOCK_PID": OWNER,
        }

    def command(self, script, *args):
        return ["/usr/bin/sandbox-exec", "-p", SANDBOX_PROFILE, "/bin/bash", str(self.root / script), *args]

    def run_script(self, script, *args, **extra):
        return subprocess.run(self.command(script, *args), env={**self.env, **extra}, text=True, capture_output=True, timeout=15)

    def held(self, name, pid=OWNER):
        slot = self.slots / ("1700000000-8888-" + name + ".slot")
        slot.mkdir()
        for field, value in {"pid": pid, "name": name, "since": "1700000000"}.items():
            (slot / field).write_text(value + "\n")
        return slot

    def test_guard_uses_recorded_owner_not_acquirer_filename(self):
        self.held("rig", OWNER)
        run = self.run_script("xcrun-slot-guard.sh", "simctl", "boot", "AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE")
        self.assertEqual(run.returncode, 0, run.stderr)
        self.assertEqual(run.stdout.strip(), "fake-xcrun")

    def test_guard_rejects_filename_ancestor_with_dead_recorded_owner(self):
        slot = self.slots / f"1700000000-{OWNER}-rig.slot"
        slot.mkdir()
        (slot / "pid").write_text(OTHER + "\n")
        (slot / "name").write_text("rig\n")
        (slot / "since").write_text("1700000000\n")
        run = self.run_script("xcrun-slot-guard.sh", "simctl", "boot", "AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE", FIXTURE_DEAD=OTHER)
        self.assertEqual(run.returncode, 75)
        self.assertEqual(run.stdout, "")

    def test_dead_owner_during_claim_is_not_granted(self):
        run = self.run_script("arbos-heavy-slot.sh", "acquire", "rig", "--pid", OWNER, "--no-wait", DIE_AFTER="1")
        self.assertEqual(run.returncode, 75, run.stdout + run.stderr)
        self.assertEqual(run.stdout, "")
        self.assertEqual(list(self.slots.glob("*.slot")), [])

    def test_reclaimer_cannot_delete_an_unpublished_claim(self):
        run = self.run_script("arbos-heavy-slot.sh", "acquire", "rig", "--pid", OWNER, "--no-wait", INJECT_RECLAIM="1")
        self.assertEqual(run.returncode, 0, run.stderr)
        slot = Path(run.stdout.strip())
        self.assertTrue(slot.is_dir(), run.stdout + run.stderr)
        self.assertEqual((slot / "pid").read_text().strip(), OWNER)

    def test_gui_concurrent_acquires_have_one_owner(self):
        env = {**self.env, "PAUSE_TAKE": "1"}
        first = subprocess.Popen(self.command("gui-lock.sh", "acquire", "first", "0"), env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        second = None
        try:
            deadline = time.monotonic() + 3
            while not list(self.root.glob("ready-*")):
                self.assertLess(time.monotonic(), deadline)
                time.sleep(.01)
            second = subprocess.Popen(self.command("gui-lock.sh", "acquire", "second", "0"), env={**env, "GUI_LOCK_PID": OTHER}, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            while len(list(self.root.glob("ready-*"))) < 2 and second.poll() is None:
                self.assertLess(time.monotonic(), deadline)
                time.sleep(.01)
            (self.root / "release-take").touch()
            first.communicate(timeout=3)
            second.communicate(timeout=3)
            self.assertEqual(sorted([first.returncode, second.returncode]), [0, 1])
        finally:
            (self.root / "release-take").touch()
            for process in [first, second]:
                if process and process.poll() is None:
                    process.kill()
                    process.communicate(timeout=5)

    def test_two_no_wait_acquirers_cannot_both_grant_over_cap(self):
        first = subprocess.Popen(self.command("arbos-heavy-slot.sh", "acquire", "rig-first", "--pid", OWNER, "--no-wait"), env={**self.env, "PAUSE_CLAIM": "1"}, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            deadline = time.monotonic() + 3
            while not (self.root / "claim-ready").exists():
                self.assertLess(time.monotonic(), deadline)
                time.sleep(.01)
            second = self.run_script("arbos-heavy-slot.sh", "acquire", "rig-second", "--pid", OTHER, "--no-wait")
            (self.root / "release-claim").touch()
            first.communicate(timeout=3)
            self.assertEqual(sorted([first.returncode, second.returncode]), [0, 75])
            self.assertEqual(len(list(self.slots.glob("*.slot"))), 1)
        finally:
            (self.root / "release-claim").touch()
            if first.poll() is None:
                first.kill()
                first.communicate(timeout=5)

    def test_guard_explicit_directory_requires_live_lease_metadata(self):
        arbitrary = self.root / "not-a-lease"
        arbitrary.mkdir()
        run = self.run_script("xcrun-slot-guard.sh", "simctl", "boot", "AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE", ARBOS_HEAVY_SLOT=str(arbitrary))
        self.assertEqual((run.returncode, run.stdout), (75, ""))

    def test_guard_rejects_invalid_owner_and_timestamp_metadata(self):
        slot = self.held("rig")
        for pid, since in [("0", "1700000000"), ("1", "1700000000"), (OWNER, "9" * 40)]:
            with self.subTest(pid=pid, since=since):
                (slot / "pid").write_text(pid + "\n")
                (slot / "since").write_text(since + "\n")
                run = self.run_script("xcrun-slot-guard.sh", "simctl", "boot", "AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE", ARBOS_HEAVY_SLOT=str(slot))
                self.assertEqual((run.returncode, run.stdout), (75, ""), run.stderr)

    def test_gui_dead_requester_is_not_granted(self):
        run = self.run_script("gui-lock.sh", "acquire", "rig", "0", FIXTURE_DEAD=OWNER)
        self.assertEqual(run.returncode, 1)
        self.assertNotIn("locked by", run.stdout)
        self.assertFalse((self.root / "gui.lock").exists())

    def test_gui_store_failure_is_not_reported_as_granted(self):
        bad = self.root / "not-directory"
        bad.write_text("synthetic obstruction")
        run = self.run_script("gui-lock.sh", "acquire", "rig", "0", GUI_LOCK_FILE=str(bad / "gui.lock"))
        self.assertEqual(run.returncode, 1)
        self.assertNotIn("locked by", run.stdout)

    def test_heavy_store_failure_does_not_allow_untracked_work(self):
        bad = self.root / "not-directory"
        bad.write_text("synthetic obstruction")
        run = self.run_script("arbos-heavy-slot.sh", "acquire", "rig", "--pid", OWNER, "--no-wait", ARBOS_HEAVY_SLOTS_DIR=str(bad))
        self.assertEqual((run.returncode, run.stdout), (75, ""))

    def test_priority_and_release_extra_capacity_are_preserved(self):
        self.held("rig-holder", OTHER)
        priority = self.run_script("arbos-heavy-slot.sh", "acquire", "gate-private", "--priority", "--pid", OWNER, "--no-wait")
        self.assertEqual(priority.returncode, 0, priority.stderr)
        self.run_script("arbos-heavy-slot.sh", "release", priority.stdout.strip())
        release = self.run_script("arbos-heavy-slot.sh", "acquire", "mac-release-private", "--pid", OWNER, "--no-wait")
        self.assertEqual(release.returncode, 0, release.stderr)

    def test_gate_queue_and_fair_turn_keep_calibrated_lease(self):
        queue = self.slots / ".gate-waiting"
        queue.mkdir()
        (queue / "900000003").write_text("900000003\n")
        refused = self.run_script("arbos-heavy-slot.sh", "acquire", "rig", "--pid", OWNER, "--no-wait")
        self.assertEqual(refused.returncode, 75)
        (self.slots / ".gate-grants").write_text("2\n")
        granted = self.run_script("arbos-heavy-slot.sh", "acquire", "rig", "--pid", OWNER, "--no-wait")
        self.assertEqual(granted.returncode, 0, granted.stderr)
        self.assertEqual((Path(granted.stdout.strip()) / "lease").read_text().strip(), "900")

    def test_p0_priority_keeps_its_lease_and_exempt_leases_stay(self):
        self.held("gate-holder", OTHER)
        (self.slots / ".p0-rigs").write_text("rig-private\n")
        granted = self.run_script("arbos-heavy-slot.sh", "acquire", "rig-private", "--pid", OWNER, "--no-wait")
        self.assertEqual(granted.returncode, 0, granted.stderr)
        self.assertEqual((Path(granted.stdout.strip()) / "lease").read_text().strip(), "1800")
        old = self.held("mac-release-old", OTHER)
        (old / "since").write_text("0\n")
        self.run_script("arbos-heavy-slot.sh", "reclaim")
        self.assertTrue(old.exists())

    def paused_gui(self, cancel_requester):
        process = subprocess.Popen(self.command("gui-lock.sh", "acquire", "rig", "0"), env={**self.env, "PAUSE_TAKE": "1"}, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            deadline = time.monotonic() + 3
            while not list(self.root.glob("ready-*")):
                self.assertLess(time.monotonic(), deadline)
                time.sleep(.01)
            if cancel_requester:
                process.terminate()
            else:
                (self.root / "dead-pids").write_text(OWNER)
            (self.root / "release-take").touch()
            process.communicate(timeout=3)
            self.assertEqual(process.returncode, -15 if cancel_requester else 1)
            self.assertFalse((self.root / "gui.lock").exists())
        finally:
            (self.root / "release-take").touch()
            if process.poll() is None:
                process.kill()
                process.communicate(timeout=5)

    def test_gui_owner_dying_during_store_is_not_granted(self):
        self.paused_gui(False)

    def test_cancelled_gui_renewal_restores_prior_inode_and_age(self):
        self.assertEqual(self.run_script("gui-lock.sh", "acquire", "rig", "0").returncode, 0)
        original = (self.root / "gui.lock").stat()
        body = (self.root / "gui.lock").read_bytes()
        self.paused_gui_renewal()
        restored = (self.root / "gui.lock").stat()
        self.assertEqual((restored.st_ino, restored.st_mtime_ns), (original.st_ino, original.st_mtime_ns))
        self.assertEqual((self.root / "gui.lock").read_bytes(), body)

    def paused_gui_renewal(self):
        process = subprocess.Popen(self.command("gui-lock.sh", "acquire", "rig", "0"), env={**self.env, "PAUSE_TAKE": "1"}, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            deadline = time.monotonic() + 3
            while not list(self.root.glob("ready-*")):
                self.assertLess(time.monotonic(), deadline)
                time.sleep(.01)
            process.terminate()
            (self.root / "release-take").touch()
            process.communicate(timeout=3)
            self.assertEqual(process.returncode, -15)
        finally:
            (self.root / "release-take").touch()
            if process.poll() is None:
                process.kill()
                process.communicate(timeout=5)

    def test_cancelled_gui_request_cannot_publish_an_orphan(self):
        self.paused_gui(True)

    def test_cancelled_heavy_claim_cannot_publish_an_orphan(self):
        process = subprocess.Popen(self.command("arbos-heavy-slot.sh", "acquire", "rig-first", "--pid", OWNER, "--no-wait"), env={**self.env, "PAUSE_CLAIM": "1"}, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            deadline = time.monotonic() + 3
            while not (self.root / "claim-ready").exists():
                self.assertLess(time.monotonic(), deadline)
                time.sleep(.01)
            process.terminate()
            (self.root / "release-claim").touch()
            process.communicate(timeout=3)
            self.assertEqual(process.returncode, 143)
            self.assertEqual(list(self.slots.glob("*.slot")), [])
        finally:
            (self.root / "release-claim").touch()
            if process.poll() is None:
                process.kill()
                process.communicate(timeout=5)

    def test_waiting_and_cancellation_hold_no_admission_mutex(self):
        self.held("rig-holder", OTHER)
        # Also pin that a queued release prevents uncapped promotion.
        queue = self.slots / ".release-waiting"
        queue.mkdir()
        (queue / "900000003").write_text("1700000000 mac-release-private\n")
        process = subprocess.Popen(self.command("arbos-heavy-slot.sh", "acquire", "rig", "--pid", OWNER), env={**self.env, "ARBOS_HEAVY_WAIT_MAX_S": "0"}, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            deadline = time.monotonic() + 3
            while not (self.root / "sleeping").exists():
                self.assertLess(time.monotonic(), deadline)
                time.sleep(.01)
            probe = subprocess.run(["/usr/bin/lockf", "-k", "-s", "-t", "0", str(self.slots / ".admission-lock"), "/usr/bin/true"])
            self.assertEqual(probe.returncode, 0)
            process.terminate()
            process.communicate(timeout=3)
            self.assertEqual(process.returncode, 143)
            self.assertEqual(list((self.slots / ".rig-waiting").glob("*")), [])
            self.assertEqual(len(list(self.slots.glob("*.slot"))), 1)
            self.assertTrue((queue / "900000003").exists())
        finally:
            (self.root / "release-sleep").touch()
            if process.poll() is None:
                process.kill()
                process.communicate(timeout=5)

    def test_uncapped_budget_grant_is_real_and_visible(self):
        self.held("rig-holder", OTHER)
        run = self.run_script("arbos-heavy-slot.sh", "acquire", "rig", "--pid", OWNER, ARBOS_HEAVY_WAIT_MAX_S="0")
        self.assertEqual(run.returncode, 0, run.stderr)
        slot = Path(run.stdout.strip())
        self.assertEqual((slot / "name").read_text().strip(), "uncapped-rig")
        self.assertEqual((slot / "pid").read_text().strip(), OWNER)

    def test_simulator_idle_threshold_and_exemption_are_preserved(self):
        slot = self.held("rig-holder", OTHER)
        (slot / "udid").write_text("AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE\n")
        (slot / "shutdown-since").write_text(str(1700000000 - 300))
        self.run_script("arbos-heavy-slot.sh", "reclaim", FIXTURE_SIM_STATE="Shutdown")
        self.assertTrue(slot.exists())
        (slot / "shutdown-since").write_text(str(1700000000 - 301))
        self.run_script("arbos-heavy-slot.sh", "reclaim", FIXTURE_SIM_STATE="Shutdown")
        self.assertFalse(slot.exists())

    def test_boot_passthrough_workflow_and_original_arguments_stay(self):
        for args, extra in [(('simctl', 'list', 'devices'), {}), (('simctl', 'bootstatus', 'synthetic', '-b'), {'GITHUB_ACTIONS': '1'})]:
            with self.subTest(args=args):
                run = self.run_script('xcrun-slot-guard.sh', *args, **extra)
                self.assertEqual((run.returncode, run.stdout.strip()), (0, 'fake-xcrun'))
                calls = [json.loads(line) for line in (self.root / 'calls.jsonl').read_text().splitlines()]
                self.assertEqual([c for c in calls if c['tool'] == 'fake-real'][-1]['args'], list(args))

    def test_no_wait_release_refusal_has_no_marker_or_signal(self):
        self.held("mac-release")
        run = self.run_script("arbos-heavy-slot.sh", "acquire", "rig", "--pid", OWNER, "--no-wait")
        self.assertEqual((run.returncode, run.stdout), (75, ""))
        self.assertFalse((self.slots / ".rig-waiting").exists())

    def test_bounded_wait_never_falls_through_uncapped(self):
        self.held("rig-holder", OTHER)
        run = self.run_script("arbos-heavy-slot.sh", "acquire", "rig", "--pid", OWNER, "--wait-max", "0", ARBOS_HEAVY_WAIT_MAX_S="0")
        self.assertEqual((run.returncode, run.stdout), (75, ""))
        self.assertEqual(list((self.slots / ".rig-waiting").glob("*")), [])
        self.assertEqual(len(list(self.slots.glob("*.slot"))), 1)

    def test_failed_expiry_cleanup_retires_boot_authority(self):
        slot = self.held("rig-holder", OWNER)
        (slot / "since").write_text("0\n")
        self.run_script("arbos-heavy-slot.sh", "reclaim", FAIL_REMOVE="1")
        self.assertTrue(slot.is_dir())
        self.assertTrue((slot / "retired").is_file())
        boot = self.run_script("xcrun-slot-guard.sh", "simctl", "boot", "AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE", ARBOS_HEAVY_SLOT=str(slot))
        self.assertEqual((boot.returncode, boot.stdout), (75, ""))

    def test_failed_retirement_and_removal_keep_capacity_reserved(self):
        slot = self.held("rig-holder", OTHER)
        (slot / "since").write_text("0\n")
        run = self.run_script("arbos-heavy-slot.sh", "acquire", "rig", "--pid", OWNER, "--no-wait", BLOCK_RETIRE_PID=OTHER, FAIL_REMOVE="1")
        self.assertEqual((run.returncode, run.stdout), (75, ""))
        self.assertTrue(slot.exists())
        self.assertEqual(len(list(self.slots.glob("*.slot"))), 1)

    def test_failed_direct_release_is_retired_and_keeps_best_effort_status(self):
        slot = self.held("rig-holder", OWNER)
        release = self.run_script("arbos-heavy-slot.sh", "release", str(slot), FAIL_REMOVE="1")
        self.assertEqual(release.returncode, 0)
        self.assertTrue((slot / "retired").is_file())
        boot = self.run_script("xcrun-slot-guard.sh", "simctl", "boot", "AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE", ARBOS_HEAVY_SLOT=str(slot))
        self.assertEqual((boot.returncode, boot.stdout), (75, ""))

    def test_invalid_held_metadata_cannot_hide_capacity(self):
        slot = self.held("rig-holder", OTHER)
        (slot / "since").write_text("invalid\n")
        run = self.run_script("arbos-heavy-slot.sh", "acquire", "rig", "--pid", OWNER, "--no-wait")
        self.assertEqual((run.returncode, run.stdout), (75, ""))
        self.assertTrue(slot.exists())
        self.assertEqual(len(list(self.slots.glob("*.slot"))), 1)

    def test_release_cannot_delete_non_slot_directory_or_mutex(self):
        unrelated = self.root / "outside-owned-slots"
        unrelated.mkdir()
        (unrelated / "sentinel").write_text("synthetic")
        run = self.run_script("arbos-heavy-slot.sh", "release", str(unrelated))
        self.assertEqual(run.returncode, 2)
        self.assertTrue((unrelated / "sentinel").exists())
        mutex = self.slots / ".admission-lock"
        mutex.touch()
        inode = mutex.stat().st_ino
        run = self.run_script("arbos-heavy-slot.sh", "release", str(mutex))
        self.assertEqual(run.returncode, 2)
        self.assertEqual(mutex.stat().st_ino, inode)

    def test_dead_unpublished_claim_is_reaped_without_touching_live_claim(self):
        dead = self.slots / f".claim-{OTHER}-synthetic"
        live = self.slots / f".claim-{OWNER}-synthetic"
        dead.mkdir()
        live.mkdir()
        self.run_script("arbos-heavy-slot.sh", "reclaim", FIXTURE_DEAD=OTHER)
        self.assertFalse(dead.exists())
        self.assertTrue(live.exists())

    def limited_path(self):
        for tool in ["python3", "dirname", "cat", "chmod", "basename", "tr", "sed", "head", "grep", "sort", "ls", "awk", "mktemp", "ln", "stat", "touch"]:
            target = self.bin / tool
            if not target.exists():
                target.symlink_to(shutil.which(tool))
        return str(self.bin)

    def test_no_lock_primitive_fails_closed_and_flock_adapter_works(self):
        path = self.limited_path()
        heavy = self.run_script("arbos-heavy-slot.sh", "acquire", "rig", "--pid", OWNER, "--no-wait", PATH=path)
        gui = self.run_script("gui-lock.sh", "acquire", "rig", "0", PATH=path)
        self.assertEqual((heavy.returncode, heavy.stdout), (75, ""))
        self.assertEqual(gui.returncode, 1)
        self.assertEqual(list(self.slots.glob("*.slot")), [])
        (self.bin / "flock").symlink_to(self.bin / "fake.py")
        heavy = self.run_script("arbos-heavy-slot.sh", "acquire", "rig", "--pid", OWNER, "--no-wait", PATH=path)
        gui = self.run_script("gui-lock.sh", "acquire", "rig", "0", PATH=path)
        self.assertEqual(heavy.returncode, 0, heavy.stderr)
        self.assertEqual(gui.returncode, 0, gui.stderr)

    def test_actual_transition_domains_reject_illegal_edges(self):
        cases = [
            ("arbos-heavy-slot.sh", 'heavy_transition New Ask; heavy_transition "Published:Asking:/synthetic.slot" OwnerStatus alive; heavy_transition "Granted:/synthetic.slot" OwnerGone', 0, "Asking\nGranted:/synthetic.slot\nGranted:/synthetic.slot\n"),
            ("arbos-heavy-slot.sh", 'heavy_transition Asking Timeout', 2, ""),
            ("arbos-heavy-slot.sh", 'lease_transition Observed OwnerAlive mac-release; lease_transition ExemptHeld Expired', 2, "ExemptHeld\n"),
            ("arbos-heavy-slot.sh", 'lease_transition RetirementFailed RemoveFailed', 0, "HeldCleanupFailed\n"),
            ("gui-lock.sh", 'gui_transition Other Acquire; gui_transition Granted Failed', 0, "Waiting\nGranted\n"),
            ("xcrun-slot-guard.sh", 'boot_transition Checking OwnedLease ""', 2, ""),
            ("xcrun-slot-guard.sh", 'boot_transition Denied OwnedLease /synthetic.slot', 0, "Denied\n"),
        ]
        for script, code, expected_code, expected_text in cases:
            with self.subTest(script=script, code=code):
                run = subprocess.run(["/usr/bin/sandbox-exec", "-p", SANDBOX_PROFILE, "/bin/bash", "-c", 'source "$1"; ' + code, "fixture", str(self.root / script)], env=self.env, text=True, capture_output=True, timeout=3)
                self.assertEqual((run.returncode, run.stdout), (expected_code, expected_text), run.stderr)

    def test_ordinary_grant_and_release_stay_compatible(self):
        run = self.run_script("arbos-heavy-slot.sh", "acquire", "rig", "--pid", OWNER, "--no-wait")
        self.assertEqual(run.returncode, 0, run.stderr)
        slot = Path(run.stdout.strip())
        self.assertEqual((slot / "pid").read_text().strip(), OWNER)
        released = self.run_script("arbos-heavy-slot.sh", "release", str(slot))
        self.assertEqual(released.returncode, 0)
        self.assertFalse(slot.exists())

    def test_gui_owner_release_contract(self):
        self.assertEqual(self.run_script("gui-lock.sh", "acquire", "first", "0").returncode, 0)
        self.assertEqual(self.run_script("gui-lock.sh", "release", "other").returncode, 1)
        self.assertEqual(self.run_script("gui-lock.sh", "release", "first").returncode, 0)


if __name__ == "__main__":
    unittest.main(verbosity=2)

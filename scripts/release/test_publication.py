"""Exercise the actual publisher with temporary artifacts and stateful local fakes."""
import json
import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest

HERE = Path(__file__).resolve().parent
SCRIPT = HERE / "publish-release.sh"
VERSION = "0.1.0"
TAG = "v" + VERSION

FAKE_GH = r'''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
p=Path(os.environ['SYNTHETIC_STATE']);state=json.loads(p.read_text());args=sys.argv[1:]
state['commands'].append(args)
options=args[:args.index('--')] if '--' in args else args
def finish(code=0, output=''):
 p.write_text(json.dumps(state));print(output) if output else None;raise SystemExit(code)
def flag(name,default=None):
 value=default
 for at,a in enumerate(options):
  if a==name:value='true' if name in ('--draft','--prerelease','--latest') else options[at+1]
  elif a.startswith(name+'='):value=a.split('=',1)[1]
 return value
if args[:1]==['api']:
 status=state.get('observation_status',200 if state['exists'] else 404)
 item={'tag_name':state.get('observed_tag',state['tag']),'draft':state['draft'],'prerelease':state['prerelease']}
 output=state.get('response_body',json.dumps(item))
 finish(0 if status==200 else 1, f'HTTP/2.0 {status} Synthetic\nContent-Type: application/json\n\n'+output)
if args[:1]==['release'] and args[1]==state.get('fail_command'):
 finish(1,'SYNTHETIC_PRIVATE_RESPONSE')
if args[:2]==['release','view']:
 tag=args[2] if len(args)>2 and not args[2].startswith('-') else None
 if tag is None:finish(output='previous')
 if tag!=state['tag'] or not state['exists']:finish(1)
 if '--json' in args:
  fields=flag('--json').split(',');data={'tagName':tag,'isDraft':state['draft'],'isPrerelease':state['prerelease'],'targetCommitish':'a'*40}
  finish(output=json.dumps({k:data[k] for k in fields}))
 finish(output='synthetic release')
if args[:2]==['release','download']:
 Path(flag('--output')).write_text('<synthetic-previous-feed/>');finish()
if args[:2]==['release','upload'] and args[2]==state['tag']:
 state['uploads']+=1;finish()
if args[:2]==['release','edit'] and args[2]==state['tag']:
 for opt,key in [('--draft','draft'),('--prerelease','prerelease'),('--latest','latest')]:
  value=flag(opt)
  if value is not None:state[key]=value=='true'
 finish()
if args[:2]==['release','create'] and args[2]==state['tag']:
 state.update(exists=True,draft=False,prerelease=flag('--prerelease','false')=='true',latest=flag('--latest','false')=='true',uploads=1);finish()
state['unexpected'].append(args);finish(97)
'''

FAKE_UPDATECTL = r'''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
p=Path(os.environ['SYNTHETIC_STATE']);s=json.loads(p.read_text());a=sys.argv[1:];s['signer_calls'].append(a[0]);p.write_text(json.dumps(s))
if a[0]=='appcast':Path(a[a.index('--out')+1]).write_text('<synthetic-verified-feed/>')
elif a[0]=='verify-appcast' and s.get('fail_verify'):raise SystemExit(12)
elif a[0]!='verify-appcast':raise SystemExit(98)
'''


class PublicationFixture:
    def run_publisher(self, *, exists=True, draft=False, prerelease=False, promote=True, observation=None,
                      dist_name="dist", direct=False, channel="stable", direct_asset=None):
        with tempfile.TemporaryDirectory(prefix="arbos-publish-state-") as directory:
            root = Path(directory)
            binary = root / "bin"
            binary.mkdir()
            # The Bash edge must use the test's interpreter, including macOS Python 3.9.
            (binary / "python3").symlink_to(sys.executable)
            for name, body in [("gh", FAKE_GH), ("updatectl", FAKE_UPDATECTL)]:
                p = binary / name
                p.write_text(body)
                p.chmod(0o755)
            for name in ["curl", "op", "cargo", "git", "ssh", "security"]:
                p = binary / name
                p.write_text("#!/bin/sh\nexit 98\n")
                p.chmod(0o755)
            state_path = root / "state.json"
            fixture_state = {
                "tag": TAG if channel == "stable" else "edge-" + VERSION,
                "exists": exists, "draft": draft, "prerelease": prerelease,
                "latest": exists and not draft and not prerelease,
                "uploads": 0, "commands": [], "unexpected": [], "signer_calls": [],
            }
            fixture_state.update(observation or {})
            state_path.write_text(json.dumps(fixture_state))
            (root / "sitecustomize.py").write_text(
                "import sys\n"
                "def no_network(event, args):\n"
                " if event in ('socket.connect','socket.getaddrinfo','socket.bind'): raise RuntimeError('network forbidden in publication tests')\n"
                "sys.addaudithook(no_network)\n"
            )
            dist = root / dist_name
            dist.mkdir()
            # Every optional array is populated so this also runs under macOS Bash 3.2.
            for name in [f"Arbos-{VERSION}-macos-arm64.tar.gz", f"Arbos-{VERSION}-macos-arm64.dmg",
                         f"arbos-mac-{VERSION}-linux-x86_64.tar.gz", f"arbos-relay-{VERSION}-linux-x86_64.tar.gz",
                         f"arbos-agentd-{VERSION}-linux-x86_64.tar.gz", f"arbosctl-{VERSION}-linux-x86_64.tar.gz",
                         "notarization-synthetic.log", "notes.md"]:
                (dist / name).write_text("synthetic fixture; never a real release payload")
            if direct_asset:
                (dist / direct_asset).write_text("synthetic operand, never a CLI option")
            command = ["/bin/bash", str(SCRIPT), "--channel", "stable", "--version", VERSION,
                       "--sha", "a" * 40, "--dist", str(dist), "--repo", "fixture/fixture",
                       "--notes-file", str(dist / "notes.md"),
                       "--key-file", str(root / "nonexistent-synthetic-key")]
            if sys.platform == "darwin":
                command = ["/usr/bin/sandbox-exec", "-p", "(version 1) (allow default) (deny network*)", *command]
            if not promote:
                command.append("--no-promote")
            if direct:
                command = [sys.executable, "-B", str(HERE / "publication.py"), "--repo", "fixture/fixture",
                           "--tag", fixture_state["tag"], "--sha", "a" * 40, "--title", "Arbos synthetic",
                           "--notes-file", str(dist / "notes.md"), "--channel", channel,
                           "--promote", str(promote).lower(), "--", direct_asset or f"Arbos-{VERSION}-macos-arm64.tar.gz"]
                if sys.platform == "darwin":
                    command = ["/usr/bin/sandbox-exec", "-p", "(version 1) (allow default) (deny network*)", *command]
            env = {"PATH": str(binary) + os.pathsep + os.environ.get("PATH", "/usr/bin:/bin"),
                   "HOME": str(root), "TMPDIR": str(root), "PYTHONNOUSERSITE": "1", "PYTHONPATH": str(root),
                   "ARBOS_UPDATECTL": str(binary / "updatectl"), "SYNTHETIC_STATE": str(state_path)}
            result = subprocess.run(command, cwd=dist if direct else root, env=env, capture_output=True, text=True)
            state = json.loads(state_path.read_text())
            self.assertEqual(state["unexpected"], [], result.stdout + result.stderr)
            if direct:
                self.assertEqual(state["signer_calls"], [], "the helper must leave signing at its parent edge")
            else:
                self.assertIn("verify-appcast", state["signer_calls"], result.stdout + result.stderr)
            return result, state


class PublicationTests(PublicationFixture, unittest.TestCase):
    def test_new_promoted_stable_release_is_published(self):
        result, state = self.run_publisher(exists=False)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(state["draft"] or state["prerelease"])
        self.assertTrue(state["latest"])
        self.assertIn("published " + TAG, result.stdout)

    def test_candidate_prerelease_can_be_refreshed_without_moving_feed(self):
        result, state = self.run_publisher(prerelease=True, promote=False)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(state["uploads"], 1)
        self.assertTrue(state["prerelease"])
        self.assertFalse(state["latest"])

    def test_existing_stable_promoted_refresh_keeps_its_publication_policy(self):
        result, state = self.run_publisher()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(state["uploads"], 1)
        self.assertFalse(state["draft"] or state["prerelease"])
        self.assertTrue(state["latest"])

    def test_no_promote_does_not_overwrite_an_existing_public_stable_feed(self):
        result, state = self.run_publisher(promote=False)
        self.assertNotEqual(result.returncode, 0, "existing stable release must refuse candidate overwrite")
        self.assertEqual(state["uploads"], 0, "--no-promote must leave already-public stable assets intact")
        self.assertFalse(any(c[:2] in (["release", "create"], ["release", "upload"], ["release", "edit"])
                             for c in state["commands"]))
        self.assertNotIn("published " + TAG, result.stdout)

    def test_successful_promoted_draft_retry_actually_becomes_public(self):
        result, state = self.run_publisher(draft=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(state["draft"], "publisher reported success while GitHub release remained a draft")
        self.assertFalse(state["prerelease"])
        self.assertTrue(state["latest"])
        self.assertIn("published " + TAG, result.stdout)

    def test_new_unpromoted_release_keeps_public_prerelease_policy(self):
        result, state = self.run_publisher(exists=False, promote=False)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(state["draft"])
        self.assertTrue(state["prerelease"])
        self.assertFalse(state["latest"])

    def test_unpromoted_existing_draft_stays_draft_and_reports_staged(self):
        result, state = self.run_publisher(draft=True, promote=False)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(state["uploads"], 1)
        self.assertTrue(state["draft"])
        self.assertFalse(state["latest"])
        self.assertIn("staged draft " + TAG, result.stdout)
        self.assertNotIn("published " + TAG, result.stdout)

    def assert_refused_without_mutation(self, observation, *, error=None):
        result, state = self.run_publisher(observation=observation)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(any(c[:2] in (["release", "create"], ["release", "upload"], ["release", "edit"])
                             for c in state["commands"]), "unreadable state attempted a publication mutation")
        self.assertNotIn("published " + TAG, result.stdout)
        self.assertNotIn("SYNTHETIC_PRIVATE_RESPONSE", result.stdout + result.stderr)
        if error:
            self.assertIn(error, result.stderr)

    def test_auth_outage_and_rate_limit_are_not_missing_releases(self):
        for status in [401, 403, 429, 500]:
            self.assert_refused_without_mutation({"observation_status": status, "response_body": "SYNTHETIC_PRIVATE_RESPONSE"})

    def test_malformed_mismatched_and_wrongly_typed_release_metadata_refuse(self):
        bodies = ["SYNTHETIC_PRIVATE_RESPONSE", "[]", '{"tag_name":"other","draft":false,"prerelease":false}',
                  '{"tag_name":"v0.1.0","draft":"false","prerelease":false}',
                  '{"tag_name":"v0.1.0","draft":false,"prerelease":false,"extra":1}',
                  '{"tag_name":"v0.1.0","draft":false,"draft":true,"prerelease":false}']
        for body in bodies:
            self.assert_refused_without_mutation({"response_body": body})

    def test_valid_json_with_oversized_whitespace_hits_byte_bound(self):
        body = '{"tag_name":"v0.1.0","draft":false,"prerelease":false}' + " " * 70000
        self.assert_refused_without_mutation({"response_body": body}, error="release_response_too_large")

    def test_signature_verification_failure_never_reaches_publication(self):
        self.assert_refused_without_mutation({"fail_verify": True})

    def test_upload_or_finalize_failure_never_reports_publication_or_updates_feed(self):
        for operation in ["upload", "edit"]:
            result, state = self.run_publisher(draft=True, observation={"fail_command": operation})
            self.assertNotEqual(result.returncode, 0)
            self.assertTrue(state["draft"])
            self.assertNotIn("published " + TAG, result.stdout)
            self.assertNotIn("SYNTHETIC_PRIVATE_RESPONSE", result.stdout + result.stderr)
            self.assertFalse(any(c[:3] == ["release", "upload", "edge"] for c in state["commands"]))

    def test_quoted_distribution_paths_are_preserved_as_arguments(self):
        result, state = self.run_publisher(exists=False, dist_name="dist folder's $(literal)")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        create = next(c for c in state["commands"] if c[:2] == ["release", "create"])
        self.assertIn("dist folder's $(literal)", create[create.index("--notes-file") + 1])


class PublicationStateTests(PublicationFixture, unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        spec = importlib.util.spec_from_file_location("release_publication_under_test", HERE / "publication.py")
        cls.machine = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = cls.machine
        spec.loader.exec_module(cls.machine)

    def test_upload_cannot_skip_finalization_or_complete_early(self):
        m = self.machine
        for observed in [m.Draft(), m.Prerelease(), m.Stable()]:
            pending = m.transition(observed, m.Request(m.Intent.PUBLISH, "stable"))
            self.assertNotIsInstance(pending, m.Complete)
            self.assertIsInstance(m.transition(pending, m.Event.FINALIZED), m.Failed)
            finalizing = m.transition(pending, m.Event.UPLOADED)
            self.assertNotIsInstance(finalizing, m.Complete)
            self.assertEqual(m.transition(finalizing, m.Event.FINALIZED), m.Complete(m.Target.STABLE))

    def test_completed_failed_and_refused_states_absorb_later_events(self):
        m = self.machine
        events = [m.Request(intent, channel) for intent in m.Intent for channel in ["stable", "edge"]]
        events += list(m.Event) + [m.Failure(m.Error.COMMAND)]
        states = [m.Complete(target) for target in m.Target] + [m.Refused(), m.Failed(m.Error.OBSERVATION)]
        for state in states:
            for event in events:
                self.assertIs(m.transition(state, event), state)

    def test_creating_a_draft_is_not_a_representable_new_release_policy(self):
        with self.assertRaises(ValueError):
            self.machine.Creating(self.machine.Target.DRAFT)

    def test_asset_operand_cannot_override_candidate_publication_flags(self):
        for exists in [False, True]:
            result, state = self.run_publisher(exists=exists, prerelease=exists, promote=False,
                                               direct=True, direct_asset="--prerelease=false")
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue(state["prerelease"], "an asset operand overrode --no-promote")
            operation = "upload" if exists else "create"
            operands = next(c for c in state["commands"] if c[:2] == ["release", operation])
            self.assertEqual(operands[operands.index("--") + 1:], ["--prerelease=false"])

    def test_existing_draft_edge_publish_uses_edge_finalization_flags(self):
        result, state = self.run_publisher(draft=True, direct=True, channel="edge")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(state["draft"])
        self.assertTrue(state["prerelease"])
        self.assertFalse(state["latest"])
        edit = next(c for c in state["commands"] if c[:2] == ["release", "edit"])
        self.assertTrue({"--draft=false", "--prerelease=true", "--latest=false"}.issubset(edit))

    def test_edge_candidate_refresh_preserves_its_unpromoted_kind(self):
        result, state = self.run_publisher(prerelease=True, promote=False, direct=True, channel="edge")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(state["draft"])
        self.assertTrue(state["prerelease"])
        self.assertFalse(state["latest"])

    def test_observation_timeout_kills_its_owned_gh_process(self):
        with tempfile.TemporaryDirectory(prefix="arbos-publication-timeout-") as directory:
            root = Path(directory)
            fake = root / "gh"
            fake.write_text("#!/usr/bin/env python3\nimport os,time\nfrom pathlib import Path\n"
                            "Path(os.environ['SYNTHETIC_PID']).write_text(str(os.getpid()))\ntime.sleep(2)\n")
            fake.chmod(0o755)
            pid = root / "pid"
            code = (
                "import sys\nsys.path.insert(0,sys.argv[1])\n"
                "from publication import run_gh,PublicationError\n"
                "try: run_gh([],timeout=0.1)\n"
                "except PublicationError as error: print(error.code.value)\n"
            )
            command = [sys.executable, "-B", "-c", code, str(HERE)]
            if sys.platform == "darwin":
                command = ["/usr/bin/sandbox-exec", "-p", "(version 1) (allow default) (deny network*)", *command]
            env = {"PATH": str(root) + os.pathsep + os.environ.get("PATH", "/usr/bin:/bin"),
                   "HOME": str(root), "SYNTHETIC_PID": str(pid), "PYTHONNOUSERSITE": "1"}
            result = subprocess.run(command, env=env, capture_output=True, text=True, timeout=5)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout.strip(), "release_command_timeout")
            if pid.exists():
                with self.assertRaises(ProcessLookupError):
                    os.kill(int(pid.read_text()), 0)

    def test_terminating_the_cli_reaps_its_owned_command_before_any_later_mutation(self):
        with tempfile.TemporaryDirectory(prefix="arbos-publication-term-") as directory:
            root = Path(directory)
            pid, release, mutation = (root / name for name in ("pid", "release", "mutation"))
            fake = root / "gh"
            fake.write_text("#!/usr/bin/env python3\nimport os,time\nfrom pathlib import Path\n"
                            "Path(os.environ['SYNTHETIC_PID']).write_text(str(os.getpid()))\n"
                            "while not Path(os.environ['SYNTHETIC_RELEASE']).exists(): time.sleep(0.01)\n"
                            "Path(os.environ['SYNTHETIC_MUTATION']).write_text('unexpected later mutation')\n")
            fake.chmod(0o755)
            notes, asset = root / "notes", root / "asset"
            notes.write_text("synthetic notes")
            asset.write_text("synthetic asset")
            command = [sys.executable, "-B", str(HERE / "publication.py"), "--repo", "fixture/fixture",
                       "--tag", TAG, "--sha", "a" * 40, "--title", "Synthetic", "--notes-file", str(notes),
                       "--channel", "stable", "--promote", "true", "--", str(asset)]
            if sys.platform == "darwin":
                command = ["/usr/bin/sandbox-exec", "-p", "(version 1) (allow default) (deny network*)", *command]
            env = {"PATH": str(root) + os.pathsep + os.environ.get("PATH", "/usr/bin:/bin"), "HOME": str(root),
                   "PYTHONNOUSERSITE": "1", "SYNTHETIC_PID": str(pid), "SYNTHETIC_RELEASE": str(release),
                   "SYNTHETIC_MUTATION": str(mutation)}
            process = subprocess.Popen(command, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            child = None
            try:
                deadline = time.monotonic() + 5
                while not pid.exists() and process.poll() is None and time.monotonic() < deadline:
                    time.sleep(0.01)
                self.assertTrue(pid.exists(), "owned fake command never reached its barrier")
                child = int(pid.read_text())
                process.terminate()
                process.communicate(timeout=5)
                release.touch()
                # A surviving command is now free to perform its fake mutation.
                with self.assertRaises(ProcessLookupError):
                    os.kill(child, 0)
                self.assertFalse(mutation.exists())
            finally:
                if process.poll() is None:
                    process.kill()
                process.communicate()
                if child is not None:
                    try:
                        os.kill(child, 9)
                    except ProcessLookupError:
                        pass


if __name__ == "__main__":
    unittest.main()

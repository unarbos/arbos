"""Selected actual controller functions; generated files and effect collaborators only."""
from pathlib import Path
import hashlib
import json
import os
import subprocess
import shlex
import signal
import shutil
import sys
import tempfile
import time
import unittest


HERE = Path(__file__).resolve().parent
TREE = HERE.parents[1]


def function(source, name):
    start = source.index(name + "() {")
    return source[start:source.index("\n}", start) + 2]


class DeployLanes(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.worker = (HERE / 'arbos-deploy-worker').read_text()
        cls.hook = (HERE / 'arbos-deploy-hook').read_text()

    def run_shell(self, body, helpers='', expected_exit=0):
        with tempfile.TemporaryDirectory(prefix='arbos-deploy-laws-') as directory:
            root = Path(directory).resolve()
            binary = root / 'bin'
            binary.mkdir()
            # Same inherited-open-description semantics as Linux flock; no services.
            if shutil.which('flock', path='/usr/bin:/bin') is None:
                lock = binary / 'flock'
                lock.write_text('#!' + sys.executable + '\nimport fcntl,sys\ntry: fcntl.flock(int(sys.argv[-1]),fcntl.LOCK_EX | (fcntl.LOCK_NB if "-n" in sys.argv else 0))\nexcept BlockingIOError: sys.exit(1)\n')
                lock.chmod(0o755)
            if sys.platform == 'darwin':
                stat = binary / 'stat'
                stat.write_text('#!' + sys.executable + '\nimport os,sys\nassert sys.argv[1:3]==["-c","%Y"]\nprint(int(os.stat(sys.argv[3]).st_mtime))\n')
                stat.chmod(0o755)
            script = root / 'fixture.sh'
            common = '. "$DEPLOY_HELPER"\n' if (HERE / 'deploy-state.sh').exists() else ''
            # Lane/smoke-only tests substitute the activation effect. The separate
            # activation suite supplies the actual function later in `helpers`.
            activation = 'activate_image_default(){ run_canary_smokes "$@"; }\n'
            script.write_text('set -euo pipefail\n' + common + 'STATE="$ROOT/state"; QUEUE="$ROOT/queue"; REPO="$ROOT/repo"; LOG="$ROOT/log"; EVENTS="$ROOT/events"\nmkdir -p "$STATE/failed" "$STATE/runs" "$QUEUE" "$REPO"\n' + activation + helpers + '\n' + body)
            env = {'PATH': str(binary) + ':/usr/bin:/bin', 'ROOT': str(root), 'TMPDIR': str(root), 'DEPLOY_HELPER': str(HERE / 'deploy-state.sh')}
            command = ['/bin/bash', str(script)]
            if sys.platform == 'darwin':
                policy = '(version 1)(allow default)(deny network*)(deny file-write*)(allow file-write* (literal "/dev/null") (subpath ' + json.dumps(str(root)) + '))(deny file-read* (subpath "/Users/Shared") (subpath "/etc/arbos") (subpath "/var/lib/arbos-deploy") (subpath "/var/lib/arbos-feedback"))'
                command = ['/usr/bin/sandbox-exec', '-p', policy, *command]
            child = subprocess.Popen(command, env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, start_new_session=True)
            try:
                output, _ = child.communicate(timeout=8)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                output, _ = child.communicate()
                self.fail('Owned controller fixture timed out: ' + output)
            finally:
                # A failed assertion can leave the deliberately queued peer waiting.
                # Retire only this fixture's newly created process group.
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            result = subprocess.CompletedProcess(command, child.returncode, output)
            self.assertEqual(result.returncode, expected_exit, result.stdout)
            return result.stdout

    def request_helpers(self):
        # The hook validates/lowercases arguments before this exact queue-writing span.
        if 'queue_request_locked()' in self.hook:
            queue = function(self.hook, 'queue_request_locked') + '\nqueue_request(){ deploy_queue_transaction "$QUEUE" queue_request_locked "$@"; }'
        else:
            tail = self.hook[self.hook.index('mkdir -p "$QUEUE"'):]
            queue = 'queue_request() ( local kind="$1" sha="$2";\n' + tail + '\n)'
        request = function(self.worker, 'handle_request')
        if 'handle_request_snapshot()' in self.worker:
            request += '\n' + function(self.worker, 'handle_request_snapshot')
        return '\n'.join([
            request,
            queue,
            'log(){ :; }; event(){ :; }; auto_enabled(){ echo 1; }; resolve(){ echo "$1"; }; on_main(){ return 0; }; newest_green(){ :; }; failed_marker(){ echo "$STATE/failed/$1-$2"; }; deployed_sha(){ :; }; relay_hotfix_marker(){ :; }; ci_state(){ echo ok; }; relay_deploy_held(){ return 1; }; canary_running(){ [[ -f "$STATE/agentd.canary" ]]; }; git(){ printf "%s\\n" aaaaaaa; }',
            'SETTLE_S=0; MAX_WAIT_S=0; RETRY_AFTER_S=600; CI_NONE_GRACE_S=900',
        ])

    def test_relay_ack_does_not_delete_new_request(self):
        self.run_shell('queue_request relay aaaaaaa >/dev/null || exit 1\ndeploy_relay(){ queue_request relay bbbbbbb >/dev/null; return 0; }\nhandle_request "$QUEUE/relay" || exit 1\nprintf "OBSERVED relay successor present=%s\\n" "$([[ -f "$QUEUE/relay" ]] && echo yes || echo no)"\n[[ -f "$QUEUE/relay" ]] && grep -qx sha=bbbbbbb "$QUEUE/relay" || exit 1', self.request_helpers())

    def test_voice_ack_does_not_delete_new_request(self):
        self.run_shell('queue_request voice aaaaaaa >/dev/null || exit 1\ndeploy_voice(){ queue_request voice bbbbbbb >/dev/null; return 0; }\nhandle_request "$QUEUE/voice" || exit 1\nprintf "OBSERVED voice successor present=%s\\n" "$([[ -f "$QUEUE/voice" ]] && echo yes || echo no)"\n[[ -f "$QUEUE/voice" ]] && grep -qx sha=bbbbbbb "$QUEUE/voice" || exit 1', self.request_helpers())

    def test_image_roll_ack_cannot_remove_a_newer_agentd_request(self):
        start = self.worker.index('  # The request is served from here on;')
        end = self.worker.index('  if quota_headroom_or_wait', start)
        fragment = self.worker[start:end]
        helpers = self.request_helpers() + '''
sed(){
  /usr/bin/sed "$@"
  if [[ "$*" == "-n s/^sha=//p $QUEUE/agentd" && ! -e "$ROOT/interleave" ]]; then
    touch "$ROOT/interleave"
    (exec 8<&-; queue_request agentd bbbbbbb >/dev/null; touch "$ROOT/published") > "$ROOT/publisher.log" 2>&1 &
    for _ in {1..10}; do [[ -e "$ROOT/published" ]] && break; sleep .01; done
  fi
}
'''
        body = 'queue_request agentd aaaaaaa >/dev/null || exit 1\nsha=aaaaaaa\nroll_ack(){\n' + fragment + '\n}\nroll_ack\nfor _ in {1..100}; do [[ -e "$ROOT/published" ]] && break; sleep .01; done\nprintf "OBSERVED publisher=%s queue=%s\\n" "$([[ -e "$ROOT/published" ]] && echo yes || echo no)" "$(/usr/bin/sed -n s/^sha=//p "$QUEUE/agentd" 2>/dev/null || echo absent)"\ncat "$ROOT/publisher.log" 2>/dev/null || :\n[[ -e "$ROOT/published" ]] && grep -qx sha=bbbbbbb "$QUEUE/agentd" || exit 1'
        self.run_shell(body, helpers)

    def test_current_success_acknowledges_current_request(self):
        self.run_shell('queue_request relay aaaaaaa >/dev/null || exit 1\ndeploy_relay(){ return 0; }\nhandle_request "$QUEUE/relay" || exit 1\n[[ ! -e "$QUEUE/relay" ]] || exit 1', self.request_helpers())

    def test_relay_source_paths_include_the_installed_shared_helper(self):
        self.run_shell('for path in $(paths_of relay); do [[ "$path" != scripts/deploy/deploy-state.sh ]] || exit 0; done\nexit 1', function(self.worker, 'paths_of'))

    def test_failure_retains_current_request(self):
        self.run_shell('queue_request relay aaaaaaa >/dev/null || exit 1\ndeploy_relay(){ return 1; }\nhandle_request "$QUEUE/relay" || :\ngrep -qx sha=aaaaaaa "$QUEUE/relay" || exit 1', self.request_helpers())

    def test_canary_and_relay_admission_are_exclusive(self):
        if 'run_canary_smokes()' in self.worker:
            helpers = self.request_helpers() + '\n' + function(self.worker, 'wait_for_relay_lane') + '\n' + function(self.worker, 'deploy_relay')
            helpers += '\ndry(){ return 1; }; relay_deploying(){ [[ -f "$STATE/relay.deploying" ]]; }; www_reloading(){ return 1; }; RELAY_SETTLE_S=0; CANARY_WAIT_MAX_S=0; LANE_MARKER_MAX_S=1800\n'
            helpers += '\ndeploy_relay_now(){ touch "$STATE/relay-effect"; }; run_canary_smokes(){ touch "$STATE/canary-effect"; [[ ! -e "$STATE/relay-effect" ]]; }; date(){ if [[ "$*" == "-u +%FT%TZ" && "${ARM:-0}" == 1 && ! -e "$ROOT/interleaved" ]]; then touch "$ROOT/interleaved"; handle_request "$QUEUE/relay"; printf synthetic-time; else /bin/date "$@"; fi; }\n'
            self.run_shell('queue_request relay aaaaaaa >/dev/null || exit 1\nARM=1\nwait_for_relay_lane aaaaaaa 0.1.1 snapshot || exit 1\n[[ -e "$STATE/canary-effect" && ! -e "$STATE/relay-effect" ]] && grep -qx sha=aaaaaaa "$QUEUE/relay" || exit 1', helpers)
            return
        start = self.worker.index('  local smoke_rc=0\n', self.worker.index('roll_image()'))
        end = self.worker.index('\n  if [[ -x', start)
        admission = self.worker[start:end]
        helpers = self.request_helpers() + '\n' + function(self.worker, 'wait_for_relay_lane')
        helpers += '\nrelay_deploying(){ [[ -f "$STATE/relay.deploying" ]]; }; www_reloading(){ return 1; }; RELAY_SETTLE_S=0; CANARY_WAIT_MAX_S=900\n'
        helpers += '\ndeploy_relay(){ printf admitted > "$STATE/relay.deploying"; return 0; }\ndate(){ if [[ "$*" == "-u +%FT%TZ" && "${ARM:-0}" == 1 ]]; then handle_request "$QUEUE/relay"; printf synthetic-time; else /bin/date "$@"; fi; }\n'
        body = 'queue_request relay aaaaaaa >/dev/null || exit 1\nversion=0.1.1; sha=aaaaaaa; ARM=1\nadmit_canary(){\n' + admission + '\n}\nadmit_canary\nprintf "OBSERVED active relay=%s active canary=%s\\n" "$([[ -f "$STATE/relay.deploying" ]] && echo yes || echo no)" "$([[ -f "$STATE/agentd.canary" ]] && echo yes || echo no)"\n[[ ! -f "$STATE/relay.deploying" || ! -f "$STATE/agentd.canary" ]] || exit 1\n'
        self.run_shell(body, helpers)

    def test_canary_deadline_defers_without_smoke(self):
        helpers = function(self.worker, 'wait_for_relay_lane')
        helpers += '\nrelay_deploying(){ return 0; }; www_reloading(){ return 1; }; log(){ :; }; sleep(){ :; }; run_canary_smokes(){ touch "$ROOT/forbidden-smoke"; }; CANARY_WAIT_MAX_S=0\n'
        self.run_shell('rc=0\nwait_for_relay_lane aaaaaaa 0.1.1 snapshot || rc=$?\nprintf "OBSERVED canary deadline status=%s\\n" "$rc"\n[[ "$rc" == 75 && ! -e "$ROOT/forbidden-smoke" ]] || exit 1', helpers)

    def test_deferred_roll_retains_request_without_switch_or_hold(self):
        start = self.worker.index('  local smoke_rc=0\n', self.worker.index('roll_image()'))
        end = self.worker.index('  # The request is served from here on;', start) if 'activate_image_default()' in self.worker else self.worker.index('  if grep -q', start)
        fragment = self.worker[start:end]
        helpers = self.request_helpers() + '\n' + function(self.worker, 'wait_for_relay_lane')
        helpers += '\nrelay_deploying(){ return 0; }; www_reloading(){ return 1; }; sleep(){ :; }; hold_agentd(){ touch "$ROOT/held"; }; run_canary_smokes(){ touch "$ROOT/smoked"; }; CANARY_WAIT_MAX_S=0\n'
        body = 'queue_request agentd aaaaaaa >/dev/null || exit 1\nsha=aaaaaaa; version=0.1.1; snapshot=synthetic\nafter_registration(){\n' + fragment + '\ntouch "$ROOT/default-switched"\n}\nafter_registration\n[[ ! -e "$ROOT/default-switched" && ! -e "$ROOT/held" && ! -e "$ROOT/smoked" ]] && grep -qx sha=aaaaaaa "$QUEUE/agentd" || exit 1'
        self.run_shell(body, helpers)

    def test_unexpected_smoke_exit_never_switches_default_or_holds_image(self):
        start = self.worker.index('  local smoke_rc=0\n', self.worker.index('roll_image()'))
        end = self.worker.index('  # The request is served from here on;', start) if 'activate_image_default()' in self.worker else self.worker.index('  if grep -q', start)
        fragment = self.worker[start:end]
        helpers = self.request_helpers() + '\n' + function(self.worker, 'wait_for_relay_lane')
        if 'run_canary_smokes()' in self.worker:
            helpers += '\n' + function(self.worker, 'run_canary_smokes')
        helpers += '\nrelay_deploying(){ return 1; }; www_reloading(){ return 1; }; log(){ :; }; record_failure(){ :; }; hold_agentd(){ touch "$ROOT/held"; }; CANARY_WAIT_MAX_S=0; RELAY_SETTLE_S=0; LANE_MARKER_MAX_S=1800; RELAY_URL=http://127.0.0.1:1; RELAY_ENV="$ROOT/not-a-config"\n'
        for host in (False, True):
            for code in (75, 127, 143):
                with self.subTest(host=host, code=code):
                    fake = '#!/bin/bash\necho attempt >> "$STATE/smoke-calls"\n' + ('[[ "${SMOKE_USER_ID:-}" == hostfixture ]] || exit 0\n' if host else '') + 'exit ' + str(code) + '\n'
                    body = 'mkdir -p "$REPO/scripts/deploy"\nprintf %s ' + shlex.quote(fake) + ' > "$REPO/scripts/deploy/agentd-canary-smoke.sh"\nchmod 700 "$REPO/scripts/deploy/agentd-canary-smoke.sh"\n'
                    body += 'sha=aaaaaaa; version=0.1.1; snapshot=synthetic; SMOKE_HOST_USER_ID=' + ('hostfixture' if host else '""') + '\n'
                    expected = 76 if code == 75 else code
                    body += 'CANARY_WAIT_MAX_S=20\nsleep(){ :; }\nafter_registration(){\n' + fragment + '\ntouch "$ROOT/default-switched"\n}\nrc=0; (after_registration) || rc=$?\nprintf "OBSERVED smoke status=%s calls=%s\\n" "$rc" "$(wc -l < "$STATE/smoke-calls")"\n[[ "$rc" == ' + str(expected) + ' && ! -e "$ROOT/default-switched" && ! -e "$ROOT/held" && "$(wc -l < "$STATE/smoke-calls" | tr -d " ")" == ' + ('2' if host else '1') + ' ]] || exit 1'
                    self.run_shell(body, helpers)

    def test_existing_smoke_success_failure_and_inconclusive_policy(self):
        start = self.worker.index('  local smoke_rc=0\n', self.worker.index('roll_image()'))
        end = self.worker.index('  # The request is served from here on;', start) if 'activate_image_default()' in self.worker else self.worker.index('  if grep -q', start)
        fragment = self.worker[start:end]
        helpers = self.request_helpers() + '\n' + function(self.worker, 'wait_for_relay_lane')
        helpers += '\n' + function(self.worker, 'run_canary_smokes')
        helpers += '\nrelay_deploying(){ return 1; }; www_reloading(){ return 1; }; record_failure(){ :; }; hold_agentd(){ touch "$ROOT/held"; }; CANARY_WAIT_MAX_S=0; RELAY_SETTLE_S=0; LANE_MARKER_MAX_S=1800; RELAY_URL=http://127.0.0.1:1; RELAY_ENV="$ROOT/not-a-config"\n'
        for host in (False, True):
            for code in (0, 1, 2):
                with self.subTest(host=host, code=code):
                    fake = '#!/bin/bash\n' + ('[[ "${SMOKE_USER_ID:-}" == hostfixture ]] || exit 0\n' if host else '') + 'exit ' + str(code) + '\n'
                    body = 'mkdir -p "$REPO/scripts/deploy"\nprintf %s ' + shlex.quote(fake) + ' > "$REPO/scripts/deploy/agentd-canary-smoke.sh"\nchmod 700 "$REPO/scripts/deploy/agentd-canary-smoke.sh"\n'
                    body += 'sha=aaaaaaa; version=0.1.1; snapshot=synthetic; SMOKE_HOST_USER_ID=' + ('hostfixture' if host else '""') + '\n'
                    body += 'after_registration(){\n' + fragment + '\ntouch "$ROOT/default-switched"\n}\nrc=0; (after_registration) || rc=$?\n'
                    body += '[[ "$rc" == 1 && -e "$ROOT/held" && ! -e "$ROOT/default-switched" ]] || exit 1' if code == 1 else '[[ "$rc" == 0 && ! -e "$ROOT/held" && -e "$ROOT/default-switched" ]] || exit 1'
                    self.run_shell(body, helpers)

    def test_existing_canary_defers_relay(self):
        self.run_shell('queue_request relay aaaaaaa >/dev/null || exit 1\nprintf canary > "$STATE/agentd.canary"\ndeploy_relay(){ touch "$ROOT/forbidden"; }\nhandle_request "$QUEUE/relay" || exit 1\n[[ ! -e "$ROOT/forbidden" ]] && grep -qx sha=aaaaaaa "$QUEUE/relay" || exit 1', self.request_helpers())

    def test_dry_tick_does_not_expire_real_markers(self):
        helpers = '\n'.join(function(self.worker, name) for name in ['marker_fresh', 'canary_running', 'relay_deploying', 'apply_pending_www_reload', 'tick'])
        helpers += '\nneed_root(){ :; }; dry(){ return 0; }; log(){ :; }; event(){ :; }; stat(){ echo 0; }; systemctl(){ echo forbidden-systemctl; return 91; }; ensure_repo(){ :; }; auto_requests(){ :; }; prune_agent_image_tags(){ :; }; continue_roll_if_due(){ :; }; flock(){ :; }; mktemp(){ /usr/bin/mktemp -d "$ROOT/copy.XXXXXX"; }; LANE_MARKER_MAX_S=1800\n'
        self.run_shell('real_state="$STATE"\nprintf stale > "$STATE/agentd.canary"\nprintf pending > "$STATE/www.reload-pending"\ntick || exit 1\nprintf "OBSERVED original canary present=%s\\n" "$([[ -f "$real_state/agentd.canary" ]] && echo yes || echo no)"\n[[ -f "$real_state/agentd.canary" ]] && [[ -f "$real_state/www.reload-pending" ]] || exit 1', helpers)

    def test_every_terminal_ack_preserves_a_superseding_request(self):
        cases = {
            'not-main': 'on_main(){ queue_request relay bbbbbbb >/dev/null; return 1; }',
            'ci-failed': 'ci_state(){ queue_request relay bbbbbbb >/dev/null; echo failed; }',
            'already-deployed': 'deployed_sha(){ queue_request relay bbbbbbb >/dev/null; echo aaaaaaa; }',
            'gave-up': 'printf attempts=2 > "$STATE/failed/relay-aaaaaaa"\nfailed_attempts(){ queue_request relay bbbbbbb >/dev/null; echo 2; }; failed_at(){ echo 0; }',
        }
        for name, interleave in cases.items():
            with self.subTest(name=name):
                self.run_shell('queue_request relay aaaaaaa >/dev/null || exit 1\n' + interleave + '\nhandle_request "$QUEUE/relay" || exit 1\ngrep -qx sha=bbbbbbb "$QUEUE/relay" || exit 1', self.request_helpers())

    def test_duplicate_sha_and_first_request_cap_are_preserved(self):
        self.run_shell('queue_request relay aaaaaaa >/dev/null || exit 1\nsed "s/^first_requested_at=.*/first_requested_at=0/" "$QUEUE/relay" > "$ROOT/old"\nmv "$ROOT/old" "$QUEUE/relay"\nqueue_request relay bbbbbbb >/dev/null || exit 1\ngrep -qx first_requested_at=0 "$QUEUE/relay" || exit 1\ncp "$QUEUE/relay" "$ROOT/copy"\nqueue_request relay bbbbbbb >/dev/null || exit 1\ncmp -s "$QUEUE/relay" "$ROOT/copy" || exit 1', self.request_helpers())

    def test_malformed_queue_cannot_reach_an_effect(self):
        valid = 'kind=relay\nsha=aaaaaaa\nrequested_at=0\nfirst_requested_at=0\nrequested_by=test\n'
        cases = [valid + 'unknown=1\n', valid + 'kind=relay\n', valid.replace('requested_at=0\n', 'requested_at=08\n', 1), valid.replace('kind=relay', 'kind=../escape'), valid.replace('requested_at=0\n', 'requested_at=$(touch forbidden)\n', 1), valid + 'x' * 5000]
        for value in cases:
            with self.subTest(value=value[:50]):
                self.run_shell('printf %s ' + shlex.quote(value) + ' > "$QUEUE/relay"\ncp "$QUEUE/relay" "$ROOT/original"\ndeploy_relay(){ touch "$ROOT/forbidden"; }\nhandle_request "$QUEUE/relay" || exit 1\n[[ ! -e "$ROOT/forbidden" ]] && cmp -s "$QUEUE/relay" "$ROOT/original" || exit 1', self.request_helpers())

    def test_live_lane_lock_outlives_expired_observation_marker(self):
        body = '''
LANE_MARKER_MAX_S=0
mkfifo "$ROOT/release"
hold(){ touch "$ROOT/ready"; read -r _ < "$ROOT/release"; }
deploy_lane_run relay hold & owner=$!
for _ in {1..100}; do [[ -e "$ROOT/ready" ]] && break; sleep .01; done
[[ -e "$ROOT/ready" ]] || exit 1
rc=0; deploy_lane_run canary touch "$ROOT/forbidden" || rc=$?
printf 'release\\n' > "$ROOT/release"
wait "$owner" || exit 1
[[ "$rc" == 75 && ! -e "$ROOT/forbidden" && ! -e "$STATE/relay.deploying" ]] || exit 1
deploy_lane_run canary touch "$ROOT/after" || exit 1
[[ -e "$ROOT/after" ]] || exit 1
'''
        self.run_shell(body)

    def test_failed_effect_releases_lease_and_marker(self):
        self.run_shell('LANE_MARKER_MAX_S=1800\nfail_effect(){ return 42; }\nrc=0; deploy_lane_run relay fail_effect || rc=$?\n[[ "$rc" == 42 && ! -e "$STATE/relay.deploying" ]] || exit 1\ndeploy_lane_run www touch "$ROOT/after" || exit 1\n[[ -e "$ROOT/after" ]] || exit 1')

    def test_lock_setup_errors_fail_closed(self):
        self.run_shell('LANE_MARKER_MAX_S=1800\nflock(){ return 77; }\nrc=0; deploy_lane_run relay touch "$ROOT/forbidden" || rc=$?\n[[ "$rc" == 76 && ! -e "$ROOT/forbidden" ]] || exit 1\nrc=0; deploy_queue_transaction "$QUEUE" touch "$ROOT/forbidden" || rc=$?\n[[ "$rc" != 0 && ! -e "$ROOT/forbidden" ]] || exit 1')

    def test_hook_waits_for_current_short_queue_transaction(self):
        body = '''
queue_request relay aaaaaaa >/dev/null || exit 1
mkfifo "$ROOT/release"
hold(){ touch "$ROOT/ready"; read -r _ < "$ROOT/release"; }
deploy_queue_transaction "$QUEUE" hold & owner=$!
for _ in {1..100}; do [[ -e "$ROOT/ready" ]] && break; sleep .01; done
queue_request relay bbbbbbb > "$ROOT/published" & publisher=$!
sleep .05
before=$(sed -n s/^sha=//p "$QUEUE/relay")
printf 'release\\n' > "$ROOT/release"
wait "$owner" && wait "$publisher" || exit 1
[[ "$before" == aaaaaaa ]] && grep -qx sha=bbbbbbb "$QUEUE/relay" || exit 1
'''
        self.run_shell(body, self.request_helpers())

    def test_cancelled_owner_releases_lease_without_touching_another_owner(self):
        body = '''
LANE_MARKER_MAX_S=1800
mkfifo "$ROOT/release"
hold(){ touch "$ROOT/ready"; read -r _ < "$ROOT/release"; }
deploy_lane_run relay hold & owner=$!
for _ in {1..100}; do [[ -e "$ROOT/ready" ]] && break; sleep .01; done
[[ -e "$ROOT/ready" ]] || exit 1
kill -TERM "$owner"
printf 'release\\n' > "$ROOT/release"
rc=0; wait "$owner" || rc=$?
# The background wrapper can exit before its leased callback finishes.
for _ in {1..100}; do [[ ! -e "$STATE/relay.deploying" ]] && break; sleep .01; done
printf 'OBSERVED cancelled_status=%s marker=%s\\n' "$rc" "$([[ -e "$STATE/relay.deploying" ]] && echo yes || echo no)"
[[ "$rc" == 143 && ! -e "$STATE/relay.deploying" ]] || exit 1
deploy_lane_run www touch "$ROOT/after" || exit 1
[[ -e "$ROOT/after" ]] || exit 1
'''
        self.run_shell(body)

    def test_refresh_reload_participates_in_the_same_lease(self):
        refresh = (HERE / 'refresh-mac-download.sh').read_text()
        helpers = function(refresh, 'reload_www') + '\n' + function(refresh, 'reload_www_now')
        helpers += '\nDEPLOY_STATE="$STATE"; LANE_MARKER_MAX_S=1800; systemctl(){ [[ "$1" != reload ]] || touch "$ROOT/reloaded"; }\n'
        self.run_shell('block_reload(){ reload_www; [[ ! -e "$ROOT/reloaded" && -e "$STATE/www.reload-pending" ]]; }\ndeploy_lane_run canary block_reload || exit 1\nreload_www || exit 1\n[[ -e "$ROOT/reloaded" && ! -e "$STATE/www.reload-pending" ]] || exit 1', helpers)


if __name__ == '__main__':
    unittest.main()

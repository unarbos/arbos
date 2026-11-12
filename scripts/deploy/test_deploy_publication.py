"""Actual automatic publishers with generated state and a local hook collaborator."""
from pathlib import Path
import ast
import importlib.util
import shlex
import unittest

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('deploy_lane_fixture', HERE / 'test_deploy_lanes.py')
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


class DeployPublication(unittest.TestCase):
    run_shell = fixture.DeployLanes.run_shell
    request_helpers = fixture.DeployLanes.request_helpers

    @classmethod
    def setUpClass(cls):
        cls.worker = (HERE / 'arbos-deploy-worker').read_text()
        cls.hook = (HERE / 'arbos-deploy-hook').read_text()

    def hook_setup(self):
        if '--observe' in self.hook:
            hook = self.hook
        else:
            # Baseline's validated inputs are fixed lowercase SHAs. This exact
            # writer entry avoids its unrelated Bash4-only lowercase expansion.
            hook = '#!/bin/bash\nset -euo pipefail\n. "${BASH_SOURCE[0]%/*}/deploy-state.sh"\nQUEUE="$ARBOS_DEPLOY_QUEUE"\n' + fixture.function(self.hook, 'queue_request_locked') + '\ndeploy_queue_transaction "$QUEUE" queue_request_locked "$@"\n'
        observer = '\ndeploy_request_installed(){ cat "$ROOT/installed-$1"; }\ndeploy_request_default(){ cat "$ROOT/default-$1"; }\n'
        return 'mkdir -p "$ROOT/install"\ncp "$DEPLOY_HELPER" "$ROOT/install/deploy-state.sh"\nprintf %s ' + shlex.quote(observer) + ' >> "$ROOT/install/deploy-state.sh"\nprintf %s ' + shlex.quote(hook) + ' > "$ROOT/install/arbos-deploy-hook"\nchmod 700 "$ROOT/install/arbos-deploy-hook"\nHOOK="$ROOT/install/arbos-deploy-hook"\nfor kind in relay voice agentd; do printf 0000000 > "$ROOT/installed-$kind"; printf 0000000 > "$ROOT/default-$kind"; done\n'


    def automatic(self, interleave, assertion, kind='relay'):
        helpers = self.request_helpers() + '\n' + fixture.function(self.worker, 'auto_requests')
        helpers += '\nnewest_green(){ echo aaaaaaa; }; paths_of(){ echo synthetic-path; }; dry(){ return 1; }; deployed_sha(){ cat "$ROOT/installed-$1"; }; auto_enabled(){ if [[ "$1" == ' + shlex.quote(kind) + ' ]]; then echo 1; else echo 0; fi; }\n'
        helpers += '\ngit(){ case "$*" in *" diff --quiet "*) interleave; return 1 ;; *" merge-base "*) return 1 ;; *) return 0 ;; esac; }\n'
        body = self.hook_setup() + '\ninterleave(){\n' + interleave + '\n}\nauto_requests\n' + assertion
        return self.run_shell(body, helpers)

    def test_pending_replacement_refuses_stale_automatic_publication(self):
        self.automatic('ARBOS_DEPLOY_QUEUE="$QUEUE" "$HOOK" relay bbbbbbb >/dev/null', 'grep -qx sha=bbbbbbb "$QUEUE/relay"')

    def test_absent_publish_ack_absent_aba_refuses_old_decision(self):
        self.automatic('ARBOS_DEPLOY_QUEUE="$QUEUE" "$HOOK" relay bbbbbbb >/dev/null\ndeploy_request_ack_commit relay bbbbbbb', '[[ ! -e "$QUEUE/relay" ]]')

    def test_installed_change_without_queue_change_refuses_old_decision(self):
        self.automatic('printf bbbbbbb > "$ROOT/installed-relay"', '[[ ! -e "$QUEUE/relay" ]]')

    def test_default_change_without_queue_change_refuses_old_decision(self):
        self.automatic('printf bbbbbbb > "$ROOT/default-agentd"', '[[ ! -e "$QUEUE/agentd" ]]', kind='agentd')

    def test_deployed_and_acknowledged_aba_refuses_old_decision(self):
        self.automatic('ARBOS_DEPLOY_QUEUE="$QUEUE" "$HOOK" relay bbbbbbb >/dev/null\nprintf bbbbbbb > "$ROOT/installed-relay"\ndeploy_request_ack_commit relay bbbbbbb', '[[ ! -e "$QUEUE/relay" ]]')

    def test_unchanged_automatic_decision_publishes(self):
        self.automatic(':', 'grep -qx sha=aaaaaaa "$QUEUE/relay"')

    def test_manual_two_argument_arrival_order_is_preserved(self):
        body = self.hook_setup() + '\nARBOS_DEPLOY_QUEUE="$QUEUE" "$HOOK" relay aaaaaaa >/dev/null\nARBOS_DEPLOY_QUEUE="$QUEUE" "$HOOK" relay bbbbbbb >/dev/null\ngrep -qx sha=bbbbbbb "$QUEUE/relay"'
        self.run_shell(body, self.request_helpers())

    def poller(self, mode, assertion):
        source = (HERE.parents[1] / 'scripts/feedback/feedback_poller.py').read_text()
        module = ast.parse(source)
        selected = {'queued_deploy_sha', 'request_deploys', 'deploy_observation'}
        lines = source.splitlines(keepends=True)
        methods = []
        for node in ast.walk(module):
            if isinstance(node, ast.FunctionDef) and node.name in selected:
                first = min([node.lineno] + [item.lineno for item in node.decorator_list])
                methods.append(''.join(lines[first - 1:node.end_lineno]))
        runner = '''from __future__ import annotations
import os,json,re,subprocess as real
from pathlib import Path
from types import SimpleNamespace
DEPLOY_QUEUE=os.environ['ARBOS_DEPLOY_QUEUE']
DEPLOY_HOOK=os.environ['HOOK']
root=Path(os.environ['ROOT'])
done=False
def log(*args): pass
def effect(mode):
    if mode in ('pending','aba'):
        result=real.run([DEPLOY_HOOK,'relay','bbbbbbb'],capture_output=True,text=True)
        assert result.returncode==0,result.stderr
    if mode=='aba':
        code='. "$DEPLOY_HELPER"; QUEUE="$ARBOS_DEPLOY_QUEUE"; deploy_request_ack_commit relay bbbbbbb'
        result=real.run(['/bin/bash','-c',code],capture_output=True,text=True)
        assert result.returncode==0,result.stderr
    if mode=='installed':(root/'installed-relay').write_text('bbbbbbb')
def run(command,**kwargs):
    global done
    if len(command)>1 and command[0]==DEPLOY_HOOK and command[1]=='--observe':
        if MODE.startswith('malformed:'):
            return real.CompletedProcess(command,0,stdout=MODE.removeprefix('malformed:'),stderr='')
        if MODE=='timeout':raise real.TimeoutExpired(command,60)
    if len(command)>2 and command[0]==DEPLOY_HOOK and command[1]=='relay' and not done:
        done=True
        effect(MODE)
    return real.run(command,**kwargs)
subprocess=SimpleNamespace(run=run,TimeoutExpired=real.TimeoutExpired)
class Poller:
''' + ''.join(methods) + '''
p=Poller()
p.gh=SimpleNamespace(contains=lambda *args:False)
p.store=SimpleNamespace(event=lambda *args,**kwargs:None)
p.request_deploys('synthetic-item',['relay'],'aaaaaaa')
'''
        runner = runner.replace('MODE', repr(mode))
        body = self.hook_setup() + '\nexport HOOK ARBOS_DEPLOY_QUEUE="$QUEUE"\nprintf %s ' + shlex.quote(runner) + ' > "$ROOT/poller.py"\npython3 -I -B "$ROOT/poller.py"\n' + assertion
        return self.run_shell(body, self.request_helpers())

    def test_poller_pending_replacement_refuses_old_decision(self):
        self.poller('pending', 'grep -qx sha=bbbbbbb "$QUEUE/relay"')

    def test_poller_absent_aba_refuses_old_decision(self):
        self.poller('aba', '[[ ! -e "$QUEUE/relay" ]]')

    def test_poller_installed_change_refuses_old_decision(self):
        self.poller('installed', '[[ ! -e "$QUEUE/relay" ]]')

    def test_poller_unchanged_decision_publishes(self):
        self.poller('none', 'grep -qx sha=aaaaaaa "$QUEUE/relay"')

    def test_poller_rejects_malformed_observations_without_fallback(self):
        for value in ('relay:0:-:-:-:extra\n', 'voice:0:-:-:-\n', 'relay:00:-:-:-\n', 'relay:0:$(touch forbidden):-:-\n', 'relay:0:-:-:-', 'relay:0:-:-:-\nextra\n', 'x' * 257):
            with self.subTest(value=value[:40]):
                self.poller('malformed:' + value, '[[ ! -e "$QUEUE/relay" ]]')

    def test_poller_observation_timeout_has_no_unconditional_fallback(self):
        self.poller('timeout', '[[ ! -e "$QUEUE/relay" ]]')

    def test_revision_survives_absent_queue_and_legacy_initialization(self):
        body = self.hook_setup() + '''
export ARBOS_DEPLOY_QUEUE="$QUEUE"
first=$("$HOOK" --observe relay)
[[ "$first" == relay:0:-:0000000:0000000 ]]
"$HOOK" relay AAAAAAA >/dev/null
next=$("$HOOK" --observe relay)
[[ "$next" == relay:1:aaaaaaa:0000000:0000000 ]]
deploy_request_ack_commit relay aaaaaaa
last=$("$HOOK" --observe relay)
[[ "$last" == relay:2:-:0000000:0000000 ]]
rc=0; "$HOOK" relay bbbbbbb "$first" >/dev/null || rc=$?
[[ "$rc" == 75 && ! -e "$QUEUE/relay" ]]
'''
        self.run_shell(body, self.request_helpers())

    def test_bad_or_exhausted_revision_never_resets_or_mutates_queue(self):
        for value in ('broken\n', '0\n\n', '999999999999999999\n'):
            with self.subTest(value=value):
                body = self.hook_setup() + '\nexport ARBOS_DEPLOY_QUEUE="$QUEUE"\n"$HOOK" relay aaaaaaa >/dev/null\nprintf %s ' + shlex.quote(value) + ''' > "$QUEUE/.relay.revision"
cp "$QUEUE/.relay.revision" "$ROOT/revision-original"
rc=0; "$HOOK" relay bbbbbbb >/dev/null || rc=$?
[[ "$rc" != 0 ]] && grep -qx sha=aaaaaaa "$QUEUE/relay" && cmp -s "$QUEUE/.relay.revision" "$ROOT/revision-original"
rc=0; deploy_request_ack_commit relay aaaaaaa || rc=$?
[[ "$rc" != 0 ]] && grep -qx sha=aaaaaaa "$QUEUE/relay"
'''
                self.run_shell(body, self.request_helpers())

    def test_conditional_cli_rejects_malformed_receipts_without_mutation(self):
        values = ['', 'relay:0:-:-', 'voice:0:-:-:-', 'relay:00:-:-:-', 'relay:0:-:-:-:extra', 'relay:0:$(touch forbidden):-:-', 'relay:9999999999999999999:-:-:-']
        for receipt in values:
            with self.subTest(receipt=receipt):
                body = self.hook_setup() + '\nrc=0; ARBOS_DEPLOY_QUEUE="$QUEUE" "$HOOK" relay aaaaaaa ' + shlex.quote(receipt) + ' > "$ROOT/result" 2>&1 || rc=$?\n[[ "$rc" == 2 && ! -e "$QUEUE/relay" && ! -e "$QUEUE/.relay.revision" ]]'
                self.run_shell(body, self.request_helpers())

    def test_revision_failure_prevents_publication_and_acknowledgement(self):
        fake = '#!/usr/bin/python3\nimport os,sys\nsource,target=sys.argv[-2:]\nif target.endswith(".relay.revision"):sys.exit(66)\nos.replace(source,target)\n'
        body = self.hook_setup() + '\nexport ARBOS_DEPLOY_QUEUE="$QUEUE"\n"$HOOK" relay aaaaaaa >/dev/null\nprintf %s ' + shlex.quote(fake) + ''' > "$ROOT/bin/mv"
chmod 700 "$ROOT/bin/mv"
rc=0; "$HOOK" relay bbbbbbb >/dev/null || rc=$?
[[ "$rc" != 0 && "$(cat "$QUEUE/.relay.revision")" == 1 ]] && grep -qx sha=aaaaaaa "$QUEUE/relay"
rc=0; deploy_request_ack_commit relay aaaaaaa || rc=$?
[[ "$rc" != 0 && "$(cat "$QUEUE/.relay.revision")" == 1 ]] && grep -qx sha=aaaaaaa "$QUEUE/relay"
'''
        self.run_shell(body, self.request_helpers())

    def test_late_publication_failure_leaves_conservative_new_revision(self):
        fake = '#!/usr/bin/python3\nimport os,sys\nsource,target=sys.argv[-2:]\nif target.endswith("/relay"):sys.exit(66)\nos.replace(source,target)\n'
        body = self.hook_setup() + '\nexport ARBOS_DEPLOY_QUEUE="$QUEUE"\n"$HOOK" relay aaaaaaa >/dev/null\nold=$("$HOOK" --observe relay)\nprintf %s ' + shlex.quote(fake) + ''' > "$ROOT/bin/mv"
chmod 700 "$ROOT/bin/mv"
rc=0; "$HOOK" relay bbbbbbb >/dev/null || rc=$?
[[ "$rc" != 0 && "$(cat "$QUEUE/.relay.revision")" == 2 ]] && grep -qx sha=aaaaaaa "$QUEUE/relay"
rm "$ROOT/bin/mv"
rc=0; "$HOOK" relay bbbbbbb "$old" >/dev/null || rc=$?
[[ "$rc" == 75 ]] && grep -qx sha=aaaaaaa "$QUEUE/relay"
'''
        self.run_shell(body, self.request_helpers())

    def test_status_observation_distinguishes_absent_and_unavailable(self):
        body = '''
[[ "$(deploy_request_status "$ROOT/missing")" == - ]]
rc=0; deploy_request_status "$ROOT/missing-parent/status" >/dev/null || rc=$?
[[ "$rc" != 0 ]]
printf invalid > "$ROOT/status"
rc=0; deploy_request_status "$ROOT/status" >/dev/null || rc=$?
[[ "$rc" != 0 ]]
printf aaaaaaa > "$ROOT/status"
[[ "$(deploy_request_status "$ROOT/status")" == aaaaaaa ]]
ln -s "$ROOT/status" "$ROOT/alias"
rc=0; deploy_request_status "$ROOT/alias" >/dev/null || rc=$?
[[ "$rc" != 0 ]]
'''
        self.run_shell(body, self.request_helpers())


if __name__ == '__main__':
    unittest.main()

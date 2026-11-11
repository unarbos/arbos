"""Actual image activation interval over local notice/HTTP/service collaborators."""
from pathlib import Path
import importlib.util
import unittest

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('deploy_lane_fixture', HERE / 'test_deploy_lanes.py')
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)
publication_spec = importlib.util.spec_from_file_location('publication_fixture', HERE / 'test_deploy_publication.py')
publication = importlib.util.module_from_spec(publication_spec)
publication_spec.loader.exec_module(publication)


class ImageActivation(unittest.TestCase):
    run_shell = fixture.DeployLanes.run_shell

    @classmethod
    def setUpClass(cls):
        cls.worker = (HERE / 'arbos-deploy-worker').read_text()
        cls.hook = (HERE / 'arbos-deploy-hook').read_text()

    def activation(self, mode, restart_status=0, conditional=False, revision_failure=False):
        start = self.worker.index('  local smoke_rc=0\n', self.worker.index('roll_image()'))
        end = self.worker.index('  # The request is served from here on;', start)
        phase = self.worker[start:end]
        helpers = fixture.function(self.worker, 'wait_for_relay_lane')
        if 'activate_image_default()' in self.worker:
            helpers += '\n' + fixture.function(self.worker, 'activate_image_default')
        helpers += '''
log(){ :; }; event(){ :; }; record_failure(){ :; }; run_canary_smokes(){ :; }
secret_from(){ printf synthetic-authority; }; sleep(){ :; }
relay_deploying(){ return 1; }; www_reloading(){ return 1; }
RELAY_SETTLE_S=0; CANARY_WAIT_MAX_S=0; LANE_MARKER_MAX_S=1800
RELAY_URL=http://127.0.0.1:1; RELAY_ENV="$ROOT/synthetic-config"
sed(){ if [[ "$1" == -i ]]; then printf 'ARBOS_DAYTONA_SNAPSHOT=%s\\n' "$snapshot" > "$RELAY_ENV"; else /usr/bin/sed "$@"; fi; }
competing(){ touch "$ROOT/competing-ready" "$ROOT/competing-live"; read -r _ < "$ROOT/release"; rm "$ROOT/competing-live"; }
start_competitor(){
  (exec 7<&-; rc=0; deploy_lane_run canary competing || rc=$?; printf '%s' "$rc" > "$ROOT/competing-done") > "$ROOT/peer.log" 2>&1 & competitor=$!
  for _ in {1..100}; do [[ -e "$ROOT/competing-ready" || -e "$ROOT/competing-done" ]] && break; /bin/sleep .01; done
}
finish_competitor(){
  if [[ -e "$ROOT/competing-ready" && ! -e "$ROOT/competing-done" ]]; then printf 'release\\n' > "$ROOT/release"; fi
  wait "$competitor"
}
relay_planned_restart(){ start_competitor; }
systemctl(){
  touch "$ROOT/restart-called"
  [[ ! -e "$ROOT/competing-live" ]] || touch "$ROOT/overlap"
  finish_competitor
  return RESTART_STATUS
}
curl(){
  case "$*" in
    */admin/snapshot/default*)
      touch "$ROOT/default-post"
      if [[ "$CONDITIONAL" == 1 ]]; then
        rc=0; ARBOS_DEPLOY_QUEUE="$QUEUE" "$HOOK" agentd bbbbbbb "$old_receipt" >/dev/null || rc=$?
        printf '%s' "$rc" > "$ROOT/publication-status"
      fi
      if [[ "$MODE" == hot ]]; then
        start_competitor
        [[ ! -e "$ROOT/competing-live" ]] || touch "$ROOT/overlap"
        finish_competitor
        printf 200
      else printf 404; fi ;;
    */healthz*) printf '{"snapshot":"synthetic-snapshot"}' ;;
    *) return 97 ;;
  esac
}
'''
        helpers = helpers.replace('RESTART_STATUS', str(restart_status))
        setup = publication.DeployPublication.hook_setup(self) if conditional else ''
        body = setup + 'CONDITIONAL=' + ('1' if conditional else '0') + '\nMODE=' + mode + '''
sha=aaaaaaa; version=0.1.1; snapshot=synthetic-snapshot
printf 'ARBOS_DAYTONA_SNAPSHOT=old\\n' > "$RELAY_ENV"
mkfifo "$ROOT/release"
'''
        if conditional:
            body += 'old_receipt=$(ARBOS_DEPLOY_QUEUE="$QUEUE" "$HOOK" --observe agentd)\n'
        if revision_failure:
            body += 'printf broken > "$QUEUE/.agentd.revision"\n'
        body += '''
image_phase(){
''' + phase + '''
}
'''
        expected = restart_status or (1 if revision_failure else 0)
        if expected:
            check = ' && ! -e "$ROOT/default-post"' if revision_failure else ''
            body += 'trap \'rc=$?; if [[ "$rc" == ' + str(expected) + ' && ! -e "$STATE/agentd.rolling.sha"' + check + ' ]]; then exit "$rc"; else exit 97; fi\' EXIT\nimage_phase\n'
        else:
            body += 'image_phase\nprintf "OBSERVED overlap=%s\\n" "$([[ -e "$ROOT/overlap" ]] && echo yes || echo no)"\n[[ ! -e "$ROOT/overlap" ]] || exit 1\ngrep -qx aaaaaaa "$STATE/agentd.rolling.sha" || exit 1\ndeploy_lane_run www touch "$ROOT/after" || exit 1\n[[ -e "$ROOT/after" ]] || exit 1\n'
            if conditional:
                body += '[[ "$(cat "$ROOT/publication-status")" == 75 && ! -e "$QUEUE/agentd" ]] || exit 1\n'
        output = self.run_shell(body, helpers, expected_exit=expected)
        if not expected:
            self.assertIn('OBSERVED overlap=no', output)

    def test_hot_default_change_remains_inside_the_canary_lease(self):
        self.activation('hot')

    def test_fallback_restart_remains_inside_the_canary_lease(self):
        self.activation('fallback')

    def test_restart_failure_never_commits_a_default_receipt(self):
        self.activation('fallback', restart_status=42)

    def test_activation_invalidates_automatic_receipt_before_default_post(self):
        self.activation('hot', conditional=True)

    def test_revision_failure_stops_before_config_default_mutation(self):
        self.activation('hot', revision_failure=True)


if __name__ == '__main__':
    unittest.main()

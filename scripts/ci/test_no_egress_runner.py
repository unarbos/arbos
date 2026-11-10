"""Subprocess contract checks; every privileged/network tool is a local fake."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


HERE = Path(__file__).resolve().parent
SCRIPT = HERE / "run-tests-no-egress.sh"

DISPATCH = r'''
import json, os, sys
from pathlib import Path
CONFIG = Path(@CONFIG@)
LOG = Path(@LOG@)
NS = Path(@NS@)
LO = Path(@LO@)
cfg = json.loads(CONFIG.read_text())
tool = Path(sys.argv[0]).name
args = sys.argv[1:]
allowed = ['PATH','RUSTFLAGS','CARGO_TERM_COLOR','CARGO_HOME','RUSTUP_HOME',
           'CC','CXX','LIBCLANG_PATH','CARGO_INCREMENTAL','HOME','USER','LOGNAME','SHELL']
with LOG.open('a') as out:
    out.write(json.dumps({'tool':tool,'args':args,'cwd':os.getcwd(),
        'namespace':NS.exists(),'loopback':LO.exists(),'env_keys':sorted(os.environ),
        'allowed_env':{k:os.environ[k] for k in allowed if k in os.environ}})+'\n')
if cfg.get('fail_tool') == tool:
    sys.exit(47)
if tool == 'setpriv' and NS.exists() and cfg.get('fail_drop'):
    sys.exit(48)
if tool == 'uname':
    print(cfg.get('system','Linux'))
elif tool == 'id':
    print(cfg.get('uid','1001') if args == ['-u'] else cfg.get('gid','1002'))
elif tool == 'cargo':
    sys.exit(cfg.get('compile_exit',0) if '--no-run' in args else cfg.get('test_exit',0))
elif tool == 'sudo':
    assert args.pop(0) == '-n'
    os.execv(args[0],args)
elif tool == 'unshare':
    assert args.pop(0) == '--net'
    NS.touch()
    os.execv(args[0],args)
elif tool == 'bash':
    os.execv('/bin/bash',['/bin/bash',*args])
elif tool == 'python3':
    assert NS.exists() and LO.exists()
    assert Path(args[0]).name == 'netns-smoke.py'
    assert args[1:4] == [cfg.get('uid','1001'),cfg.get('gid','1002'),'--']
    os.execv(args[4],args[4:])
elif tool == 'ip':
    assert NS.exists()
    assert args == ['link','set','dev','lo','up']
    LO.touch()
elif tool == 'setpriv':
    if args == ['--help']:
        print('--reset-env' if cfg.get('reset_supported',True) else 'old setpriv')
        sys.exit(0)
    split = next(i for i,arg in enumerate(args) if not arg.startswith('--'))
    options, command = args[:split], args[split:]
    env = dict(os.environ)
    if '--reset-env' in options:
        env = {'HOME':'/passwd/runner-home','USER':'runner','LOGNAME':'runner',
               'SHELL':'/bin/bash','PATH':'/usr/bin:/bin'}
    os.execve(command[0],command,env)
else:
    raise AssertionError(tool)
'''


class RunnerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="mock-", dir=HERE)
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bin = self.root / "tools with spaces"
        self.bin.mkdir()
        self.workspace = self.root / "workspace with spaces"
        self.workspace.mkdir()
        (self.workspace / "Cargo.toml").write_text("[workspace]\n")
        (self.workspace / "Cargo.lock").write_text("version = 4\n")
        self.config = self.root / "config.json"
        self.log = self.root / "commands.jsonl"
        self.ns = self.root / "namespace"
        self.lo = self.root / "loopback"
        self.settings = {}
        self.config.write_text("{}")
        code = DISPATCH
        for key, value in {"CONFIG":self.config,"LOG":self.log,"NS":self.ns,"LO":self.lo}.items():
            code = code.replace("@"+key+"@", repr(str(value)))
        for tool in ("uname","id","cargo","sudo","unshare","ip","setpriv","bash","python3"):
            path = self.bin / tool
            path.write_text(f"#!{sys.executable}\n" + code)
            path.chmod(0o755)
        (self.bin / "env").symlink_to("/usr/bin/env")
        self.marker = self.root / "startup-must-not-run"
        self.startup = self.root / "untrusted-bash-env"
        self.startup.write_text(f"printf executed > {str(self.marker)!r}\n")
        self.env = {
            "PATH":str(self.bin), "RUSTFLAGS":"-D warnings", "CARGO_TERM_COLOR":"always",
            "OPENAI_API_KEY":"synthetic", "ARBOS_MATRIX_TEST_HS":"https://example.invalid",
            "ARBOS_MATRIX_TEST_AS_TOKEN":"synthetic", "ARBOS_AVATAR_SMOKE":"1",
            "HTTP_PROXY":"http://example.invalid", "DOCKER_HOST":"unix:///synthetic",
            "SUDO_UID":"0", "HOME":"/untrusted/parent-home",
        }

    def run_helper(self, *arguments):
        self.config.write_text(json.dumps(self.settings))
        # Set BASH_ENV only after this trusted caller starts. The helper must clear it
        # before invoking its privileged shell; no startup file is executed by this test.
        caller = 'export BASH_ENV="$1"; shift; script=$1; shift; source "$script" "$@"'
        return subprocess.run(
            ["/bin/bash","--noprofile","--norc","-c",caller,"contract-caller",
             str(self.startup),str(SCRIPT),*arguments], cwd=self.workspace, env=self.env,
            text=True, capture_output=True, timeout=10,
        )

    def records(self, tool=None):
        records = [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []
        return [r for r in records if tool is None or r["tool"] == tool]

    def test_shell_syntax(self):
        self.assertEqual(subprocess.run(["/bin/bash","-n",str(SCRIPT)],capture_output=True).returncode,0)

    def test_full_suite_compiles_then_runs_offline_in_loopback_namespace(self):
        result = self.run_helper()
        self.assertEqual(result.returncode,0,result.stderr)
        cargo = self.records("cargo")
        self.assertEqual([r["args"] for r in cargo], [
            ["test","--workspace","--locked","--no-run"],
            ["test","--workspace","--locked","--offline"],
        ])
        self.assertFalse(cargo[0]["namespace"])
        self.assertTrue(cargo[1]["namespace"] and cargo[1]["loopback"])
        self.assertTrue(all(r["cwd"] == str(self.workspace) for r in cargo))
        tools = [r["tool"] for r in self.records()]
        self.assertLess(tools.index("cargo"),tools.index("sudo"))
        self.assertLess(tools.index("unshare"),tools.index("ip"))
        self.assertEqual(len(self.records("ip")),1)
        self.assertEqual(len(self.records("python3")),1)
        self.assertLess(tools.index("ip"),tools.index("python3"))
        self.assertEqual(tools[-1],"cargo")

    def test_privilege_drop_uses_runner_identity_clears_groups_and_caps(self):
        result = self.run_helper()
        self.assertEqual(result.returncode,0,result.stderr)
        dropped = [r for r in self.records("setpriv") if r["namespace"]]
        self.assertEqual(len(dropped),1)
        args = dropped[0]["args"]
        for required in ("--reuid=1001","--regid=1002","--clear-groups","--no-new-privs",
                         "--inh-caps=-all","--ambient-caps=-all","--bounding-set=-all","--reset-env"):
            self.assertIn(required,args)
        self.assertNotIn("--keep-groups",args)
        self.assertNotIn("--init-groups",args)
        self.assertEqual(self.records("sudo")[0]["args"][0],"-n")

    def test_environment_is_reset_before_root_shell_and_cargo(self):
        result = self.run_helper()
        self.assertEqual(result.returncode,0,result.stderr)
        forbidden = {"BASH_ENV","OPENAI_API_KEY","ARBOS_MATRIX_TEST_HS","ARBOS_MATRIX_TEST_AS_TOKEN",
                     "ARBOS_AVATAR_SMOKE","HTTP_PROXY","DOCKER_HOST","SUDO_UID"}
        for record in self.records("unshare") + self.records("bash") + self.records("cargo"):
            self.assertFalse(forbidden.intersection(record["env_keys"]),record)
        cargo = self.records("cargo")
        self.assertEqual(cargo[0]["allowed_env"],cargo[1]["allowed_env"])
        self.assertEqual(cargo[1]["allowed_env"]["HOME"],"/passwd/runner-home")
        self.assertFalse(self.marker.exists())

    def test_compiler_cache_settings_are_identical_and_literal(self):
        self.env.update({"CC":"/llvm/bin/clang","CXX":"/llvm/bin/clang++",
                         "LIBCLANG_PATH":"/llvm/lib", "CARGO_INCREMENTAL":"0",
                         "CARGO_HOME":"/cache/cargo with spaces","RUSTUP_HOME":"/cache/rustup"})
        self.env["RUSTFLAGS"] = f"literal ; touch {self.marker}"
        result = self.run_helper()
        self.assertEqual(result.returncode,0,result.stderr)
        cargo = self.records("cargo")
        self.assertEqual(cargo[0]["allowed_env"],cargo[1]["allowed_env"])
        for name in ("CC","CXX","LIBCLANG_PATH","CARGO_INCREMENTAL","CARGO_HOME","RUSTUP_HOME","RUSTFLAGS"):
            self.assertEqual(cargo[1]["allowed_env"][name],self.env[name])
        self.assertFalse(self.marker.exists())

    def test_compile_failure_never_enters_namespace_or_runs_tests(self):
        self.settings["compile_exit"] = 31
        self.assertEqual(self.run_helper().returncode,31)
        self.assertEqual(len(self.records("cargo")),1)
        self.assertEqual(self.records("sudo"),[])
        self.assertEqual(self.records("unshare"),[])

    def test_test_exit_status_is_preserved(self):
        self.settings["test_exit"] = 42
        self.assertEqual(self.run_helper().returncode,42)

    def test_privilege_setup_failure_never_executes_tests(self):
        self.settings["fail_drop"] = True
        self.assertEqual(self.run_helper().returncode,48)
        self.assertEqual(len(self.records("cargo")),1)

    def test_failed_namespace_or_loopback_setup_never_falls_back(self):
        for tool in ("sudo","unshare","ip","python3"):
            with self.subTest(tool=tool):
                self.settings["fail_tool"] = tool
                self.log.unlink(missing_ok=True)
                self.ns.unlink(missing_ok=True)
                self.lo.unlink(missing_ok=True)
                self.assertEqual(self.run_helper().returncode,47)
                self.assertEqual(len(self.records("cargo")),1)

    def test_root_or_invalid_identity_is_refused_before_compilation(self):
        for name,value in (("uid","0"),("gid","0"),("uid","not-a-uid")):
            with self.subTest(name=name,value=value):
                self.settings = {name:value}
                self.log.unlink(missing_ok=True)
                self.assertNotEqual(self.run_helper().returncode,0)
                self.assertEqual(self.records("cargo"),[])

    def test_non_linux_is_refused_before_compilation(self):
        self.settings["system"] = "Darwin"
        self.assertNotEqual(self.run_helper().returncode,0)
        self.assertEqual(self.records("cargo"),[])

    def test_missing_tools_are_refused_before_compilation(self):
        for name in ("cargo","sudo","unshare","ip","setpriv","env","bash","python3"):
            with self.subTest(tool=name):
                path = self.bin/name
                hidden = self.bin/(name+".hidden")
                path.rename(hidden)
                self.log.unlink(missing_ok=True)
                try:
                    self.assertNotEqual(self.run_helper().returncode,0)
                    self.assertEqual(self.records("cargo"),[])
                finally:
                    hidden.rename(path)

    def test_old_setpriv_is_refused_before_compilation(self):
        self.settings["reset_supported"] = False
        self.assertNotEqual(self.run_helper().returncode,0)
        self.assertEqual(self.records("cargo"),[])

    def test_workspace_and_unexpected_arguments_are_refused(self):
        self.assertNotEqual(self.run_helper("--ignored").returncode,0)
        (self.workspace/"Cargo.lock").unlink()
        self.assertNotEqual(self.run_helper().returncode,0)
        self.assertEqual(self.records("cargo"),[])


if __name__ == "__main__":
    unittest.main()

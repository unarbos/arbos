"""Exercise actual std-only OS handoff through owned fake executables; never open an app."""
from pathlib import Path
import argparse, hashlib, json, os, shlex, subprocess, sys, tempfile, time, unittest
HERE=Path(__file__).resolve().parent
PARSER=argparse.ArgumentParser();PARSER.add_argument('--root',type=Path,default=HERE.parents[1]);ARGS,UNIT_ARGS=PARSER.parse_known_args();ROOT=ARGS.root.resolve()
SOURCE=ROOT/'crates/arbos-mac/src/external_open.rs'
class ExternalOpenTests(unittest.TestCase):
 @classmethod
 def setUpClass(cls):
  cls.temporary=tempfile.TemporaryDirectory(prefix='arbos-external-open-');cls.work=Path(cls.temporary.name);cls.source_hash=hashlib.sha256(SOURCE.read_bytes()).hexdigest();cls.binary=cls.work/'open-driver'
  driver=cls.work/'driver.rs';driver.write_text('#[path = '+json.dumps(str(SOURCE))+'] mod external_open;\nfn main(){let args:Vec<_>=std::env::args_os().collect();let path=std::path::Path::new(&args[2]); match args[1].to_str().unwrap(){"normal"=>external_open::open_with_system(path,false),"quick"=>external_open::open_with_system(path,true),"reveal"=>external_open::reveal(path),_=>panic!("invalid fixture action")}}\n')
  env={'PATH':os.environ.get('PATH','/usr/bin:/bin'),'HOME':str(cls.work),'RUSTUP_HOME':os.environ.get('RUSTUP_HOME',str(Path.home()/'.rustup'))}
  compiled=subprocess.run(['rustc','--edition=2021','-D','warnings',str(driver),'-o',str(cls.binary)],env=env,capture_output=True,text=True,timeout=30)
  if compiled.returncode:raise AssertionError('Actual native handoff module did not compile: '+compiled.stderr)
  cls.fake=cls.work/'fake';cls.fake.mkdir();cls.empty=cls.work/'empty';cls.empty.mkdir()
  observer=cls.work/'observe.py';observer.write_text('import json,os,stat,sys\nfrom pathlib import Path\nn=os.stat("/dev/null")\ndef null(fd):\n s=os.fstat(fd);return stat.S_ISCHR(s.st_mode)and s.st_rdev==n.st_rdev\nrecord={"program":Path(sys.argv[1]).name,"args":sys.argv[2:],"stdout_null":null(1),"stderr_null":null(2)}\nPath(os.environ["ARBOS_FAKE_OPEN_LOG"]).write_text(json.dumps(record))\nprint("synthetic-open-stdout",flush=True)\nprint("synthetic-open-stderr",file=sys.stderr,flush=True)\n')
  for program in ['open','qlmanage','xdg-open']:
   path=cls.fake/program;path.write_text('#!/bin/sh\nexec '+shlex.quote(sys.executable)+' -I -B '+shlex.quote(str(observer))+' "$0" "$@"\n');path.chmod(0o700)
 @classmethod
 def tearDownClass(cls):
  try:assert hashlib.sha256(SOURCE.read_bytes()).hexdigest()==cls.source_hash,'Product source changed during check'
  finally:cls.temporary.cleanup()
 def run_action(self,action,path,available=True):
  record=self.work/('record-'+self._testMethodName+'.json');record.unlink(missing_ok=True)
  env={'PATH':str(self.fake if available else self.empty),'HOME':str(self.work),'ARBOS_FAKE_OPEN_LOG':str(record)}
  result=subprocess.run([str(self.binary),action,str(path)],cwd=self.work,env=env,capture_output=True,text=True,timeout=5)
  self.assertEqual(result.returncode,0)
  if not available:return result,None
  deadline=time.monotonic()+3
  while not record.exists()and time.monotonic()<deadline:time.sleep(0.01)
  self.assertTrue(record.exists(),'Owned fake executable did not report a launch')
  return result,json.loads(record.read_text())
 def assert_launch(self,action,path):
  result,record=self.run_action(action,path);mac=sys.platform=='darwin'
  program='qlmanage'if mac and action=='quick'else('open'if mac else'xdg-open')
  args=([('-p'if action=='quick'else'-R'),str(path)]if mac and action!='normal'else[(os.path.dirname(str(path))if action=='reveal'and not mac else str(path))])
  muted=not mac or action=='quick'
  self.assertEqual(record,{'program':program,'args':args,'stdout_null':muted,'stderr_null':muted})
  self.assertEqual(result.stdout,''if muted else'synthetic-open-stdout\n');self.assertEqual(result.stderr,''if muted else'synthetic-open-stderr\n')
 def test_default_application_exact_argv_and_stdio(self):self.assert_launch('normal',self.work/"saved 'report' 雪.pdf")
 def test_quick_look_exact_argv_and_stdio(self):self.assert_launch('quick',self.work/'look with spaces.pdf')
 def test_reveal_exact_argv_and_stdio(self):self.assert_launch('reveal',self.work/'nested'/'saved file.txt')
 def test_shell_characters_are_one_literal_argument(self):
  sentinel=self.work/'unexpected-shell-effect';path=self.work/'$(touch unexpected-shell-effect); $VALUE.txt';self.assert_launch('normal',path);self.assertFalse(sentinel.exists())
 def test_relative_reveal_fallback(self):self.assert_launch('reveal',Path('saved-file'))
 def test_missing_open_remains_best_effort_and_reports_failure(self):
  path=self.work/'missing.pdf';result,record=self.run_action('normal',path,False);self.assertIsNone(record);self.assertEqual(result.stdout,'');self.assertIn('open '+str(path)+': ',result.stderr)
 def test_missing_reveal_remains_best_effort_and_reports_failure(self):
  path=self.work/'missing.pdf';result,record=self.run_action('reveal',path,False);self.assertIsNone(record);self.assertEqual(result.stdout,'');self.assertIn('reveal '+str(path)+': ',result.stderr)
if __name__=='__main__':unittest.main(argv=[sys.argv[0]]+UNIT_ARGS,verbosity=2)

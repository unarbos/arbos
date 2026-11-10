"""Negative controls mutate only selected real source in disposable local trees."""
from pathlib import Path
import argparse, base64, hashlib, json, os, re, shutil, subprocess, tempfile, time
HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
LAW = 'harness/arbos-pi-ext/pending-owner-laws.test.ts'
BASE = '.improve/state-machine-checks/pending-owners/'
SOURCES = ['crates/arbos-agentd/src/approval.rs', 'crates/arbos-agentd/src/store.rs', 'crates/arbos-core/src/approval.rs', 'crates/arbos-core/src/ids.rs', 'crates/arbos-relay/src/state.rs', 'crates/arbos-relay/src/restart_lifetime.rs']
FILES = SOURCES + ['Cargo.lock', LAW, 'harness/arbos-pi-ext/package.json'] + [BASE + name for name in ['extract.py', 'native/Cargo.toml', 'native/Cargo.lock', 'native/src/main.rs', 'native/src/delivery_driver.rs']]
CASES = [
 ('closed-inline-stays-inline', 94001, SOURCES[0], 'if matches!(delivery, WaitDelivery::Inline(sender) if sender.is_closed()) {', 'if false {'),
 ('ignored-claim-commits', 94003, SOURCES[1], 'if claimed != 1 {', 'if claimed > 1 {'),
 ('same-user-separate-mutex', 94004, SOURCES[4], 'Some(slot) => slot,', 'Some(_slot) => Arc::new(tokio::sync::Mutex::new(())),'),
 ('owner-clears-independent-marker', 94005, SOURCES[4], 'markers.remove(&self.permit.user_id);', 'markers.clear();'),
]
def sha(path): return hashlib.sha256(path.read_bytes()).hexdigest()
def main():
 parser = argparse.ArgumentParser(); parser.add_argument('--output', required=True, type=Path); args = parser.parse_args()
 output = args.output.resolve(); output.mkdir(parents=True, exist_ok=False)
 hashes = {name: sha(ROOT/name) for name in FILES}
 rows = []
 # All effects are local compile/SQLite/mutex observations; the parent gate supplies its no-egress fence.
 for name, seed, source, before, after in CASES:
  with tempfile.TemporaryDirectory(prefix='arbos-pending-mutant-') as raw:
   tree = Path(raw)
   for relative in FILES:
    dest = tree/relative; dest.parent.mkdir(parents=True, exist_ok=True); shutil.copyfile(ROOT/relative, dest)
   (tree/'harness/arbos-pi-ext/node_modules').symlink_to((ROOT/'harness/arbos-pi-ext/node_modules').resolve())
   target = tree/BASE/'native/target'
   seed_target = HERE/'native/target'
   if os.uname().sysname == 'Darwin':
    subprocess.run(['/bin/cp', '-cR', str(seed_target), str(target)], check=True, capture_output=True)
   else:
    shutil.copytree(seed_target, target)
   product = tree/source; text = product.read_text(); assert text.count(before) == 1, (name, 'ambiguous source')
   product.write_text(text.replace(before, after, 1))
   row = {'name': name, 'seed': seed, 'source': source, 'original_sha256': hashes[source], 'mutant_sha256': sha(product)}
   for replay in [False, True]:
    # Rustup/Cargo cache roots are build-only; no token/proxy/config/opt-in environment is inherited.
    environment = {'PATH': os.environ.get('PATH', '/usr/bin:/bin'), 'CARGO_HOME': os.environ['CARGO_HOME'], 'RUSTUP_HOME': os.environ['RUSTUP_HOME'], 'ARBOS_PENDING_LAW': str(seed)}
    if replay: environment['ARBOS_PENDING_REPLAY'] = json.dumps(row['shrunk_input'], separators=(',', ':'))
    start = time.monotonic(); result = subprocess.run(['node', '--test', str(tree/LAW)], cwd=tree, env=environment, capture_output=True, text=True, timeout=120)
    log = (result.stdout + result.stderr).replace(str(tree), '<isolated-tree>')
    key = 'replay' if replay else 'generated'; (output/f'{name}-{key}.log').write_text(log)
    detected = result.returncode != 0 and ('Exact counterexample replay' in log if replay else 'pending-owner-counterexample:' in log and 'Property falsified' in log)
    row[key] = {'exit': result.returncode, 'seconds': time.monotonic()-start, 'detected': detected}
    assert detected, (name, key, log[-4000:])
    if not replay:
     encoded = re.search(r'pending-owner-counterexample:([A-Za-z0-9+/=]+)', log).group(1)
     details = json.loads(base64.b64decode(encoded)); row['shrunk_input'] = details['result']['shrunkInput']
     row['generated']['formatted_failure'] = re.search(r'Property falsified[^\n]*', log).group(0)
     assert details['result']['shrunkInput'] == details['replay']['shrunkInput']
     (output/f'{name}-counterexample.json').write_text(json.dumps(details, indent=2)+'\n')
   rows.append(row)
 assert hashes == {name: sha(ROOT/name) for name in FILES}
 record = {'original_sources_unchanged': True, 'method': 'actual production source mutant, regenerate selected definitions, clean isolated leaf package, compile and run seeded Effect law plus exact shrunk replay', 'sources': hashes, 'results': rows}
 (output/'RESULTS.json').write_text(json.dumps(record, indent=2)+'\n'); print(json.dumps({'mutants': len(rows), 'generated_detected': len(rows), 'replay_detected': len(rows), 'source_unchanged': True}))
if __name__ == '__main__': main()

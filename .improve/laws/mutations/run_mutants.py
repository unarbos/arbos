#!/usr/bin/env python3
"""Compile isolated source mutations; reuse exact generated laws and replay shrinking.

Requires the finalized law suite's dependencies and unmodified release bridge.
No product or .improve/laws source is edited. Only local temporary copies differ.
"""
import argparse
import difflib
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
LAWS = ROOT / '.improve/laws'
NATIVE = LAWS / 'native'
REDACT = ROOT / 'crates/arbos-core/src/redact.rs'
URL = ROOT / 'crates/arbos-mac/src/ui/urlwrap.rs'


def run(command, **kwargs):
    result = subprocess.run(command, capture_output=True, text=True, **kwargs)
    if result.returncode:
        raise RuntimeError(f'{command[0]} failed ({result.returncode}):\n{result.stdout}{result.stderr}')
    return result


def replace_function(source, signature, body):
    start = source.index(signature)
    end = source.index('\n}', start) + 2
    return source[:start] + signature + '\n    ' + body + '\n}' + source[end:]


def mutate_json(source):
    old = '''        Value::String(s) => {
            let clean = scrub(s);
            if clean != *s {
                *s = clean;
            }
        }'''
    if source.count(old) != 1:
        raise ValueError('Expected exact actual JSON string-leaf branch')
    return source.replace(old, '        Value::String(s) => s.clear(),')


def main(only=None):
    original = {'redact': REDACT.read_text(), 'url': URL.read_text()}
    specs = [
        ('redaction-identity', 'redact', lambda s: replace_function(s, 'pub fn scrub(text: &str) -> String {', 'text.to_owned()'), 'confident-token-removal'),
        ('url-drop-visible-character', 'url', lambda s: replace_function(s, 'pub fn hint_bare_urls(text: &str) -> String {', 'text.chars().skip(1).collect()'), 'url-visible-text-roundtrip'),
        ('json-empty-strings', 'redact', mutate_json, 'json-scrub-compositionality'),
        ('redaction-empty-output', 'redact', lambda s: replace_function(s, 'pub fn redact_patterns(text: &str) -> String {', 'String::new()'), 'prose'),
    ]
    if only is not None:
        specs = [spec for spec in specs if spec[0] == only]
        if not specs:
            raise ValueError('Unknown isolated mutation selector')
    results = {'source_sha256': {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest() for path in (REDACT, URL)},
               'isolation': 'Only temporary source copies mutated; original product and law files unchanged.', 'mutants': []}
    target = HERE / 'target'
    for name, changed, mutation, law in specs:
        with tempfile.TemporaryDirectory(prefix='source-mutant-', dir=HERE) as tmp:
            tmp = Path(tmp)
            versions = dict(original)
            versions[changed] = mutation(versions[changed])
            for key, source in versions.items():
                (tmp / f'{key}.rs').write_text(source)
            (HERE / f'{name}.diff').write_text(''.join(difflib.unified_diff(original[changed].splitlines(keepends=True), versions[changed].splitlines(keepends=True), fromfile=f'actual/{changed}.rs', tofile=f'temporary-mutant/{changed}.rs')))
            manifest = (NATIVE / 'Cargo.toml').read_text().replace('../../../crates/arbos-core/src/redact.rs', 'redact.rs').replace('src/main.rs', 'bridge.rs')
            (tmp / 'Cargo.toml').write_text(manifest)
            shutil.copyfile(NATIVE / 'Cargo.lock', tmp / 'Cargo.lock')
            bridge = (NATIVE / 'src/main.rs').read_text()
            path_line = '#[path = "../../../../crates/arbos-mac/src/ui/urlwrap.rs"]'
            if bridge.count(path_line) != 1:
                raise ValueError('Expected direct actual URL source binding')
            (tmp / 'bridge.rs').write_text(bridge.replace(path_line, '#[path = "url.rs"]'))
            started = time.perf_counter()
            build = run(['cargo', 'build', '--manifest-path', str(tmp / 'Cargo.toml'), '--target-dir', str(target), '--release', '--offline', '--locked', '--jobs', '2'], cwd=ROOT)
            (HERE / f'{name}-build.log').write_text(build.stderr.replace(str(tmp), '<temporary-source>').replace(str(ROOT), '<repository>'))
            check = run(['node', str(HERE / 'check.ts'), str(NATIVE / 'target/release/laws-bridge'), str(target / 'release/laws-bridge'), law, str(HERE / f'{name}.json')], cwd=ROOT)
            summary = json.loads(check.stdout)
            summary.update(name=name, seconds=round(time.perf_counter() - started, 3))
            results['mutants'].append(summary)
            print(json.dumps(summary), flush=True)
    assert results['source_sha256'] == {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest() for path in (REDACT, URL)}
    output = 'RESULTS.json' if only is None else f'RESULTS-{only}.json'
    (HERE / output).write_text(json.dumps(results, indent=2) + '\n')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--only', help='rerun one named mutation without replacing the four-control summary')
    main(parser.parse_args().only)

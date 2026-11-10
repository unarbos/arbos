"""Analyze only the immutable P6 hosted logs; no execution of downloaded content."""
from pathlib import Path
import collections
import hashlib
import json
import re
import subprocess

HERE = Path(__file__).resolve().parent
P6 = '84cb882feefc327e67167b723c9441015ab65944'
BASE = '2865cb29b806121d4a90f55ba6ce151a150279a0'
P8 = 'b15e3f9ba8080539a9f882cc806478f3ba6a8f33'


def git(*args):
    return subprocess.check_output(['git', *args], text=True)


def digest(text):
    return hashlib.sha256(text.encode()).hexdigest()


def method(text, name):
    found = list(re.finditer(r'^    (?:private |static )?func ' + name + r'\(', text, re.M))
    assert len(found) == 1, (name, len(found))
    start = found[0].start()
    return text[start:text.index('\n    }', found[0].end()) + 6]


def counts(cases):
    return {s: sum(row[1] == s for row in cases) for s in ['passed', 'failed', 'skipped']}


def main():
    inspection = json.loads((HERE / 'ZIP-INSPECTION.json').read_text())
    assert inspection['head_sha'] == P6 and inspection['github_digest_match']
    for entry in inspection['entries']:
        raw = (HERE / 'raw' / entry['name']).read_bytes()
        assert len(raw) == entry['bytes'] and hashlib.sha256(raw).hexdigest() == entry['sha256']
    text = (HERE / 'raw/xcodebuild-test.log').read_text()
    build = (HERE / 'raw/xcodebuild-test-build.log').read_text()
    unit = re.findall(r"Test case '([^']+)' (passed|failed|skipped)\b", text)
    ui = re.findall(r"Test Case '([^']+)' (passed|failed|skipped) \(([0-9.]+) seconds\)\.", text)
    assert len(unit) == len(set(name for name, _ in unit)) == 551
    assert len(ui) == len(set(row[0] for row in ui)) == 68
    assert text.count("Test case '") == len(unit), 'Unparsed/interleaved unit record'
    assert counts(unit) == {'passed': 550, 'failed': 1, 'skipped': 0}
    assert counts(ui) == {'passed': 54, 'failed': 1, 'skipped': 13}
    summary = re.search(r"Test Suite 'ArbosUITests\.xctest' failed[^\n]*\n\s*Executed (\d+) tests, with (\d+) tests skipped and (\d+) failure \(0 unexpected\) in ([0-9.]+)", text)
    assert summary and [int(summary[i]) for i in [1, 2, 3]] == [68, 13, 1]
    assert '** TEST BUILD SUCCEEDED **' in build and '** TEST EXECUTE FAILED **' in text
    unit_failed = 'AddMembersActionTests.testAnOldRelayWithoutTheBatchRouteGetsThePerItemCalls()'
    ui_failed = '-[ArbosUITests.ChatSearchResultJumpUITests testAResultTapOnTheSenderAvatarAlsoStaysInTheChat]'
    assert dict(unit)[unit_failed] == 'failed'
    assert next(row[1] for row in ui if row[0] == ui_failed) == 'failed'
    console = (HERE / 'job-private.log').read_text()
    job = json.loads((HERE / 'JOB.json').read_text())
    assert hashlib.sha256((HERE / 'job-private.log').read_bytes()).hexdigest() == job['console_sha256']
    assert job['id'] == 129873404857 and job['head_sha'] == P6 and job['conclusion'] == 'failure'
    test_step = [step for step in job['steps'] if step['name'] == 'fastlane test']
    assert len(test_step) == 1 and test_step[0]['number'] == 11 and test_step[0]['conclusion'] == 'failure'
    assert '--all-targets' in console and '--all-targets --verify' in console
    assert "wrote ['ARBOS_RELAY_URL'] for 2 test target(s)" in console
    assert "verified ['ARBOS_RELAY_URL'] for 2 test target(s)" in console
    assert 'ARBOS_RELAY_URL=http://127.0.0.1:1' in console
    assert 'AddMembersActionTests' not in console, 'Reinspect unit details if console expands'
    sources = {}
    for path in ['apps/ios/Arbos/ArbosTests/AddMembersActionTests.swift',
                 'apps/ios/Arbos/ArbosUITests/ChatSearchResultJumpUITests.swift',
                 'apps/ios/Arbos/Arbos/UI/ChatView.swift',
                 'apps/ios/Arbos/Arbos/UI/Transcript.swift']:
        data = {tag: git('show', ref + ':' + path) for tag, ref in [('BASE', BASE), ('P6', P6), ('P8', P8)]}
        assert len(set(data.values())) == 1, path
        sources[path] = {'whole_source_sha256': {tag: digest(s) for tag, s in data.items()}, 'all_byte_equal': True}
    methods = {}
    for path, names in [
        ('apps/ios/Arbos/Arbos/Store/AppModel.swift', ['installRelayForTests', 'addMembers']),
        ('apps/ios/Arbos/Arbos/Sync/RelayAPI.swift', ['request', 'requestOnce', 'addMembers', 'addMember', 'addAgent'])]:
        a, b = [git('show', ref + ':' + path) for ref in [BASE, P6]]
        methods[path] = {}
        for name in names:
            left, right = method(a, name), method(b, name)
            assert left == right, (path, name)
            methods[path][name] = {'BASE_sha256': digest(left), 'P6_sha256': digest(right), 'byte_equal': True}
    changed = git('diff', '--name-only', BASE, P6, '--', 'apps/ios').splitlines()
    unit_fail_match = re.search(re.escape(unit_failed) + r"' failed on [^\n]+ \(([0-9.]+) seconds\)", text)
    assert unit_fail_match
    errors = [line for line in text.splitlines() if ': error:' in line and 'ArbosUITests.' in line]
    assert len(errors) == 1 and 'ChatSearchResultJumpUITests.swift:204:' in errors[0]
    case = '-[ArbosUITests.ChatSearchRevealLandingUITests testAResultOutsideTheWindowLandsAfterTheFix]'
    assert next(row[1] for row in ui if row[0] == case) == 'passed'
    result = {
        'run': {'id': 42828800182, 'head_sha': P6, 'attempt': 1, 'status': 'completed', 'conclusion': 'failure'},
        'job': {'id': job['id'], 'name': 'build-xcframework + fastlane test', 'failed_step': '11 fastlane test', 'started_at': job['started_at'], 'completed_at': job['completed_at']},
        'build_for_testing': 'passed',
        'unit': {'counts': counts(unit), 'total_unique_case_records': len(unit), 'failed_cases': [unit_failed],
                 'failed_case_seconds': float(unit_fail_match[1]), 'assertion_detail_available': False,
                 'count_method': 'Unique complete lowercase per-case text records; no interleaving repair, unit-target aggregate or xcresult is supplied.'},
        'ui': {'counts': counts(ui), 'total_unique_case_records': len(ui), 'failure_events': 1, 'suite_seconds': float(summary[4]),
               'failed_cases': [ui_failed], 'skipped_cases': [row[0] for row in ui if row[1] == 'skipped'],
               'assertion': {'source': 'ChatSearchResultJumpUITests.swift:204', 'expectation': 'The tapped message is realized in the transcript.',
                             'observed': 'After tapping sender avatar on long-412, the message is not realized; visible rows are g6–g9.'},
               'search_reveal_case_status': 'passed; this is not the earlier shared search-reveal failure method'},
        'test_plan_fence': {'written_targets': 2, 'verified_targets': 2, 'relay_override': 'http://127.0.0.1:1',
                            'console_sha256': job['console_sha256'], 'scope': 'Actual job console write/verify proof, not whole-simulator no-egress certification.'},
        'source_comparison': {'BASE': BASE, 'P6': P6, 'P8': P8, 'exact_sources': sources, 'direct_method_bodies': methods, 'P6_changed_iOS_paths': changed},
        'classification': [
            'An actual historical test failure after successful build and both targets fenced; not runner-capacity or zero-test evidence.',
            'The legacy add-members fallback unit/test and direct request methods equal BASE. Uploaded text omits the failed assertion; the cause is unresolved, not proven flaky/setup or a fixed P6 defect.',
            'The sender-avatar result-jump UI assertion is distinct from the prior search-reveal case, which passed here. Its test/ChatView/Transcript bytes equal BASE/P8, but other P6 model/API changes remain; the root cause is unresolved.',
            'Later corrected integrated sources exercise these cases separately; they do not rewrite this immutable head or prove its historical cause.',
            'No old P6 rerun, new test/app run, source/ref/index mutation or speculative fix was performed.'],
    }
    (HERE / 'SUMMARY.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({'run': result['run'], 'unit': result['unit']['counts'], 'ui': result['ui']['counts'], 'source_equal_checks': len(sources), 'unchanged_method_checks': sum(map(len, methods.values()))}))


if __name__ == '__main__':
    main()

#!/usr/bin/env python3
"""Bounded pure transition oracle; never source a controller's effectful top level."""
import json
import os
from pathlib import Path
import re
import selectors
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[3]
FILES = {'deploy-state.sh': ('deploy_request_transition', 'deploy_lane_transition', 'deploy_receipt_valid', 'deploy_publication_state')}

SAFE = re.compile(r'^[A-Za-z0-9_./:-]{0,512}$')


def definition(text, name):
    matches = list(re.finditer(r'^' + re.escape(name) + r'\(\) \{', text, re.M))
    if len(matches) != 1:
        raise ValueError('Expected one actual pure definition')
    start = matches[0].start()
    line_end = text.index('\n', start)
    if text[start:line_end].rstrip().endswith('}'):
        return text[start:line_end] + '\n'
    end = text.index('\n}', line_end) + 2
    if text[end:end + 1] not in ('\n', ''):
        raise ValueError('Unexpected pure function layout')
    return text[start:end] + '\n'


def program():
    parts = []
    for filename, names in FILES.items():
        text = (ROOT / 'scripts/deploy' / filename).read_text()
        parts.extend(definition(text, name) for name in names)
    # All data travels as positional read fields. No eval, files, PIDs, clocks,
    # xcrun or controller dispatch is available in this restricted empty-PATH shell.
    return '\n'.join(parts) + r'''
enable -n command builtin exec eval source . hash kill enable
while IFS='|' read -r machine state count; do
  printf '0|%s\n' "$state"
  for ((i=0; i<count; i++)); do
    IFS='|' read -r event argument || exit 2
    case "$machine" in
      request) next=$(deploy_request_transition "$state" "$event"); status=$? ;;
      lane) next=$(deploy_lane_transition "$state" "$event"); status=$? ;;
      publication) next=$(deploy_publication_state "$event" "$argument"); status=$? ;;
      *) exit 2 ;;
    esac
    state=$next
    printf '%s|%s\n' "$status" "$state"
  done
done
'''


class Oracle:
    def __init__(self):
        command = ['/bin/bash', '--noprofile', '--norc', '--restricted', '-c', program()]
        if sys.platform == 'darwin':
            policy = '(version 1) (allow default) (deny network*) (deny file-write*) (deny file-read* (subpath "/Users/Shared")) (deny process-exec) (allow process-exec (literal "/bin/bash"))'
            command = ['/usr/bin/sandbox-exec', '-p', policy, *command]
        self.child = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                      stderr=subprocess.DEVNULL, env={'PATH': ''})
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.child.stdout, selectors.EVENT_READ)
        self.buffer = b''

    def line(self, deadline):
        while b'\n' not in self.buffer:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not self.selector.select(remaining):
                raise RuntimeError('Pure transition timeout')
            chunk = os.read(self.child.stdout.fileno(), 8192)
            if not chunk:
                raise RuntimeError('Pure transition process stopped')
            self.buffer += chunk
            if len(self.buffer) > 262144:
                raise RuntimeError('Pure transition response too large')
        line, self.buffer = self.buffer.split(b'\n', 1)
        return line.decode('ascii')

    def request(self, machine, initial, events):
        lines = [f'{machine}|{initial}|{len(events)}']
        for event in events:
            lines.append(event['event'] + '|' + (event.get('args') or [''])[0])
        self.child.stdin.write(('\n'.join(lines) + '\n').encode('ascii'))
        self.child.stdin.flush()
        deadline = time.monotonic() + 5
        observations = []
        for _ in range(len(events) + 1):
            fields = self.line(deadline).split('|')
            if len(fields) != 2:
                raise RuntimeError('Invalid pure transition response')
            status, state = fields
            observation = {'state': state, 'status': int(status)}
            observations.append(observation)
        return observations[0], observations[1:]

    def close(self):
        self.selector.close()
        try:
            self.child.stdin.close()
            self.child.wait(timeout=1)
        except (BrokenPipeError, subprocess.TimeoutExpired):
            self.child.kill()
            self.child.wait()
        finally:
            self.child.stdout.close()


def validate(item):
    if not isinstance(item, dict) or set(item) != {'id', 'machine', 'initial', 'events'}:
        raise ValueError('Invalid trace')
    if type(item['id']) is not int or not 0 <= item['id'] <= 2**53 - 1:
        raise ValueError('Invalid trace')
    if item['machine'] not in ('request', 'lane', 'publication') or not isinstance(item['initial'], str) or not SAFE.fullmatch(item['initial']):
        raise ValueError('Invalid trace')
    if not isinstance(item['events'], list) or len(item['events']) > 64:
        raise ValueError('Invalid trace')
    for event in item['events']:
        if not isinstance(event, dict) or set(event) not in ({'event'}, {'event', 'args'}) or not isinstance(event['event'], str) or not SAFE.fullmatch(event['event']):
            raise ValueError('Invalid trace')
        if item['machine'] == 'publication':
            args = event.get('args')
            if not isinstance(args, list) or len(args) != 1 or not isinstance(args[0], str) or not SAFE.fullmatch(args[0]):
                raise ValueError('Invalid trace')
        elif 'args' in event:
            raise ValueError('Invalid trace')


def audit(event, args):
    if event.startswith('socket.'):
        raise PermissionError('No network in pure state oracle')
    if event == 'subprocess.Popen' and args[0] not in ('/bin/bash', '/usr/bin/sandbox-exec'):
        raise PermissionError('Only the restricted pure shell is allowed')
    if event == 'open' and isinstance(args[0], (str, bytes)):
        path = os.fsdecode(args[0])
        name = Path(path).name.lower()
        if path.startswith('/Users/Shared/') or name == '.env' or name.startswith('.env.') or name.endswith(('.pem', '.key')) or 'keychain' in path.lower():
            raise PermissionError('Protected path')


def main():
    sys.addaudithook(audit)
    oracle = Oracle()
    try:
        while True:
            line = sys.stdin.buffer.readline(65538)
            if not line:
                break
            identifier = None
            try:
                if len(line) > 65536:
                    if not line.endswith(b'\n'):
                        while tail := sys.stdin.buffer.readline(65538):
                            if tail.endswith(b'\n'):
                                break
                    raise ValueError('Invalid trace')
                item = json.loads(line)
                if isinstance(item, dict) and type(item.get('id')) is int:
                    identifier = item['id']
                validate(item)
                initial, steps = oracle.request(item['machine'], item['initial'], item['events'])
                reply = {'id': item['id'], 'initial': initial, 'steps': steps}
            except (ValueError, TypeError, KeyError, RecursionError):
                reply = {'id': identifier, 'error': 'Invalid trace'}
            print(json.dumps(reply, separators=(',', ':')), flush=True)
    finally:
        oracle.close()


if __name__ == '__main__':
    signal.signal(signal.SIGTERM, lambda number, _frame: sys.exit(128 + number))
    main()

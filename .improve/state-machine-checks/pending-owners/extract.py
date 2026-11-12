"""Compile selected actual ownership definitions; never rewrite their policy bodies."""
from pathlib import Path
import argparse, hashlib, json, re

FILES = {
    'delivery': 'crates/arbos-agentd/src/approval.rs',
    'store': 'crates/arbos-agentd/src/store.rs',
    'outcome': 'crates/arbos-core/src/approval.rs',
    'ids': 'crates/arbos-core/src/ids.rs',
    'creation': 'crates/arbos-relay/src/state.rs',
    'restart': 'crates/arbos-relay/src/restart_lifetime.rs',
}

def block(text, anchor, derive=False):
    if text.count(anchor) != 1:
        raise ValueError('Source anchor missing or ambiguous')
    start = text.index(anchor)
    if derive:
        start = text.rfind('#[derive(', 0, start)
        if start < 0:
            raise ValueError('Missing actual derive')
    opening = text.index('{', text.index(anchor))
    index, depth = opening, 0
    while index < len(text):
        if text.startswith('//', index):
            end = text.find('\n', index)
            index = len(text) if end < 0 else end + 1
            continue
        if text.startswith('/*', index):
            end = text.find('*/', index + 2)
            if end < 0:
                raise ValueError('Unterminated source comment')
            index = end + 2
            continue
        if text[index] == '"':
            index += 1
            while index < len(text):
                if text[index] == '\\':
                    index += 2
                elif text[index] == '"':
                    index += 1
                    break
                else:
                    index += 1
            continue
        if text[index] == '{':
            depth += 1
        elif text[index] == '}':
            depth -= 1
            if depth == 0:
                return text[start:index + 1]
        index += 1
    raise ValueError('Unbalanced selected source')

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    root, output = args.root.resolve(), args.output.resolve()
    data = {}
    hashes = {}
    for key, name in FILES.items():
        path = root / name
        raw = path.read_bytes()
        if len(raw) > 2_000_000:
            raise ValueError('Source size exceeds bound')
        data[key] = raw.decode('utf-8')
        hashes[name] = hashlib.sha256(raw).hexdigest()
    pieces = {}
    def take(key, anchor, derive=False):
        source = block(data[key], anchor, derive)
        pieces[key + ':' + anchor] = hashlib.sha256(source.encode()).hexdigest()
        return source
    outcome = take('outcome', 'pub enum ApprovalOutcome', True)
    delivery = '\n\n'.join([
        take('delivery', 'pub enum WaitDelivery', True),
        take('delivery', 'enum DeliveryEvent', True),
        take('delivery', 'enum DeliveryAction', True),
        take('delivery', 'fn transition_delivery('),
    ])
    store = data['store']
    schema_match = re.search(r'const SCHEMA: &str = r#"[\s\S]*?"#;', store)
    if not schema_match:
        raise ValueError('Actual schema not found')
    schema = schema_match.group(0)
    migration = '"ALTER TABLE approvals ADD COLUMN grant_consumed_at INTEGER"'
    if store.count('c.execute_batch(' + migration + ')') != 1:
        raise ValueError('Actual grant migration missing')
    tuple_match = re.search(r'#\[derive\(Clone\)\]\s*pub struct Store\(Arc<Mutex<Connection>>\);', store)
    if not tuple_match:
        raise ValueError('Actual Store tuple missing')
    methods = '\n\n'.join(take('store', name) for name in [
        'pub(crate) fn with<T>', 'pub fn insert_approval(', 'pub fn approval(',
        'pub fn consume_grant(', 'pub fn decide_approval(',
    ])
    row = take('store', 'pub struct ApprovalRow', True)
    row_reader = take('store', 'fn approval_row(')
    creation = '\n\n'.join([
        take('creation', 'pub struct RestartSignal {'),
        take('creation', 'impl Default for RestartSignal'),
        'impl RestartSignal {\n' + take('creation', 'pub(crate) async fn admit_creation(') + '\n}',
        take('creation', 'pub(crate) struct CreationPermit {'),
        take('creation', 'impl CreationPermit {'),
        take('creation', "pub(crate) struct CreatingGuard<'a>"),
        take('creation', "impl<'a> CreatingGuard<'a>"),
        take('creation', "impl Drop for CreatingGuard<'_>"),
    ])
    now = take('ids', 'pub fn now_ms()')
    restart = json.dumps(str(root / FILES['restart']), ensure_ascii=False)
    generated = '''// Generated from actual checked source; edits belong in the original modules.\n#![allow(dead_code)]\n'''
    generated += 'pub mod outcome { use serde::{Serialize,Deserialize};\n' + outcome + '\n}\n'
    generated += 'pub mod delivery { use tokio::sync::oneshot; use super::outcome::ApprovalOutcome;\n' + delivery + '\ninclude!("delivery_driver.rs");\n}\n'
    generated += 'pub mod store { use std::sync::{Arc,Mutex}; use anyhow::Result; use rusqlite::{params,Connection,OptionalExtension}; use serde::{Serialize,Deserialize}; use serde_json::Value; use crate::ids::now_ms;\n'
    generated += tuple_match.group(0) + '\n' + schema + '\n' + row + '\n' + row_reader + '\nimpl Store {\n' + methods + '\n}\n'
    generated += 'impl Store { pub fn fresh() -> Result<Self> { let c=Connection::open_in_memory()?; c.execute_batch(SCHEMA)?; c.execute_batch(' + migration + ')?; Ok(Self(Arc::new(Mutex::new(c)))) } }\n}\n'
    generated += 'pub mod creation { use std::collections::HashMap; use std::sync::Arc;\n' + creation + '\n}\n'
    # now_ms and the restart controller are also actual source, with no fake clock/policy.
    generated += 'pub mod ids {\n' + now + '\n}\n'
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(generated)
    (output.parent / 'restart_path.rs').write_text('#[allow(dead_code)]\n#[path = ' + restart + ']\npub mod restart_lifetime;\n')
    provenance = {'sources': hashes, 'selected_items': pieces,
                  'schema_sha256': hashlib.sha256(schema.encode()).hexdigest(),
                  'generated_sha256': hashlib.sha256(generated.encode()).hexdigest(),
                  'method': 'literal actual definitions; only imports/modules and in-memory fixture construction added'}
    output.with_suffix('.provenance.json').write_text(json.dumps(provenance, indent=2) + '\n')
    print(json.dumps({'sources': len(hashes), 'items': len(pieces), 'generated_sha256': provenance['generated_sha256']}))

if __name__ == '__main__':
    main()

"""Extract narrow SQLite inputs from actual Rust source; reject changed query shapes."""
from pathlib import Path
import hashlib
import re

ROOT = Path(__file__).resolve().parents[2]
DB = ROOT / 'crates/arbos-relay/src/db.rs'
PEOPLE = ROOT / 'crates/arbos-relay/src/db_people.rs'


def raw_constant(text, name):
    matches = re.findall(r'\b' + re.escape(name) + r':\s*&str\s*=\s*r#"(.*?)"#;', text, re.S)
    if len(matches) != 1:
        raise ValueError(f'Expected one raw constant {name}')
    return matches[0]


def body(text, name):
    start = text.index('pub fn ' + name + '(')
    end = text.find('\n    pub fn ', start + 1)
    return text[start:end if end >= 0 else len(text)]


def selects(text):
    return re.findall(r'"(SELECT [^"\\]*)"', text, re.S)


def extract():
    db, people = DB.read_text(), PEOPLE.read_text()
    state = selects(body(db, 'room_state'))
    # room_state may share the newly introduced position helper.
    position = re.findall(r'\bROOM_POSITION_SQL:\s*&str\s*=\s*"([^"\\]+)";', db)
    if len(state) == 2 and len(position) == 1:
        state.insert(0, position[0])
    if len(state) != 3 or not state[0].startswith('SELECT head, floor FROM room_seq') or not state[2].startswith('SELECT COUNT(*), COALESCE(SUM(LENGTH(bytes)),0) FROM rows'):
        raise ValueError('room_state SQL shape changed; update the fixture explicitly')
    cols = re.search(r'const MEMBER_COLS:\s*&str\s*=\s*"([^"\\]+)";', people).group(1)
    members, chats = selects(body(people, 'members_of')), selects(body(people, 'chats_of_member'))
    if len(members) != 1 or len(chats) not in (1, 2) or '{MEMBER_COLS}' not in members[0]:
        raise ValueError('People query shape changed')
    batched = chats[1] if len(chats) == 2 else None
    if batched is not None and not (
        batched.startswith('SELECT {MEMBER_COLS} FROM people_members')
        and 'WHERE left_at IS NULL AND chat_id IN (' in batched
        and 'WHERE m.member = ?1 AND m.left_at IS NULL AND p.deleted_at IS NULL' in batched
        and 'ORDER BY chat_id, joined_at, CASE WHEN member LIKE' in batched
    ):
        raise ValueError('Batched roster SQL shape changed; review the fixture explicitly')
    migrations = []
    for table in ('devices', 'projects'):
        found = re.findall(r'"(ALTER TABLE ' + table + r" ADD COLUMN user_id TEXT NOT NULL DEFAULT 'u_default')\"", db)
        if len(found) != 1:
            raise ValueError('Required ownership migration changed')
        migrations.extend(found)
    return {'schema': raw_constant(db, 'SCHEMA'), 'people_schema': raw_constant(people, 'PEOPLE_SCHEMA'),
            'migrations': migrations, 'project_name_index': re.search(r'"(CREATE UNIQUE INDEX IF NOT EXISTS projects_user_name ON projects\(user_id, name\))"', db).group(1), 'room_state': state, 'room_position': position[0] if len(position) == 1 else None,
            'members_of': members[0].replace('{MEMBER_COLS}', cols), 'chats_of_member': chats[0],
            'batched_rosters': batched.replace('{MEMBER_COLS}', cols) if batched is not None else None,
            'source_sha256': {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest() for p in (DB, PEOPLE)}}

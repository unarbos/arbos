"""Synthetic relational fixtures only. No existing database is accepted."""
import sqlite3

SCALES = (1, 10, 100, 1000)
PAYLOAD = b'synthetic-opaque-row:' + b'x' * 44  # 65 bytes; not a valid Loro update.


def seed(path, source, experiment, scale):
    if experiment not in ('history', 'registry') or scale not in SCALES or path.exists():
        raise ValueError('Fixture requires a fresh path and a declared experiment/scale')
    rooms = 10 if experiment == 'history' else 10 * scale
    per_room = 100 * scale if experiment == 'history' else 100
    c = sqlite3.connect(path)
    c.executescript(source['schema'] + source['people_schema'])
    for sql in source['migrations']:
        c.execute(sql)
    c.execute(source['project_name_index'])
    c.execute('PRAGMA journal_mode=OFF')
    c.execute('PRAGMA synchronous=OFF')
    # A shared owner sees every room: this deliberately stresses the unbounded registry.
    users = ['u_owner'] + [f'u_peer_{i}' for i in range(6)]
    c.executemany('INSERT INTO users(id,created_at) VALUES (?,1)', [(u,) for u in users])
    c.executemany('INSERT INTO devices(id,name,token_hash,created_at,user_id) VALUES (?,?,?,1,?)',
                  [(f'd_{i}', 'Synthetic device', f'synthetic-hash-{i}', u) for i, u in enumerate(users)])
    for i in range(rooms):
        chat, project, room = f'c_{i:05}', f'p_{i:05}', f'people/c_{i:05}'
        group = i % 2 == 1
        c.execute('INSERT INTO projects(id,name,created_at,last_active_at,user_id) VALUES (?,?,1,1,?)', (project, f'Synthetic project {i}', 'u_owner'))
        c.execute('INSERT INTO chats(id,project_id,created_at) VALUES (?,?,?)', (f'pc_{i:05}', project, i))
        c.execute('INSERT INTO people_chats(id,kind,created_by,created_at) VALUES (?,?,?,?)', (chat, 'group' if group else 'dm', 'u_owner', i))
        roster = (['user:u_owner'] + [f'user:u_peer_{n}' for n in range(6)] + [f'agent:{project}']) if group else ['user:u_owner', 'user:u_peer_0']
        c.executemany('INSERT INTO people_members(chat_id,member,role,added_by,joined_at) VALUES (?,?,?,?,1)',
                      [(chat, m, 'agent' if m.startswith('agent:') else 'owner' if m == 'user:u_owner' or not group else 'member', 'u_owner') for m in roster])
        c.execute('INSERT INTO project_members(project_id,user_id,role,added_by,joined_at) VALUES (?,?,?,?,1)', (project, 'u_owner', 'owner', 'u_owner'))
        c.execute('INSERT INTO people_reads(chat_id,device_id,seq,at) VALUES (?,?,?,1)', (chat, 'd_0', per_room // 4 + per_room // 2))
        # Retain exactly per_room rows above a compacted floor; opaque checkpoint is SQL-only.
        c.execute('INSERT INTO room_seq(room,head,floor) VALUES (?,?,?)', (room, per_room + per_room // 4, per_room // 4))
        c.execute('INSERT INTO checkpoints(room,seq,frontier,bytes,size,created_at) VALUES (?,?,?,?,?,1)', (room, per_room // 4, b'synthetic-frontier', b'synthetic-checkpoint', 20))
        c.executemany('INSERT INTO rows(room,seq,device,batch_id,bytes,created_at) VALUES (?,?,?,?,?,?)',
                      ((room, n, 'd_0', f'b_{n}', PAYLOAD, n) for n in range(per_room // 4 + 1, per_room // 4 + per_room + 1)))
    c.commit()
    counts = {table: c.execute(f'SELECT COUNT(*) FROM {table}').fetchone()[0] for table in ('users','devices','projects','chats','people_chats','people_members','project_members','people_reads','room_seq','checkpoints','rows')}
    assert counts['rows'] == 1000 * scale
    assert counts['people_members'] == rooms * 5  # half DM2, half group8.
    c.close()
    if path.stat().st_size > 500 * 1024**2:
        raise RuntimeError('Fixture exceeded 500MiB budget')
    return {'experiment': experiment, 'scale': scale, 'rooms': rooms, 'rows_per_room': per_room, 'counts': counts, 'database_bytes': path.stat().st_size}

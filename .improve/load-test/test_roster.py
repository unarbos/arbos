"""Small fresh-data regression checks; no timing runs or existing DB access."""
import copy
from pathlib import Path
import sqlite3
import tempfile
import unittest

from fixture import seed
from roster import roster_rows, sources, verify


class RosterTests(unittest.TestCase):
    def fixture(self):
        directory = tempfile.TemporaryDirectory(prefix='arbos-roster-check-')
        self.addCleanup(directory.cleanup)
        path = Path(directory.name) / 'synthetic.db'
        before, after = sources()
        seed(path, before, 'registry', 1)
        return path, before, after

    def test_complete_ordered_rows_match_for_human_agent_and_missing(self):
        path, before, after = self.fixture()
        hashes = verify(path, before, after)
        self.assertEqual(len(hashes), 4)
        self.assertNotEqual(hashes['user:u_owner'], hashes['agent:p_00001'])

    def test_deleted_left_and_foreign_rosters_remain_outside_scope(self):
        path, before, after = self.fixture()
        with sqlite3.connect(path) as connection:
            connection.execute("UPDATE people_chats SET deleted_at=2 WHERE id='c_00001'")
            connection.execute("UPDATE people_members SET left_at=2 WHERE chat_id='c_00000' AND member='user:u_owner'")
            connection.execute("UPDATE people_members SET member='user:foreign' WHERE chat_id='c_00002' AND member='user:u_owner'")
            connection.commit()
        verify(path, before, after)
        with sqlite3.connect(path) as connection:
            rows = roster_rows(connection, after, 'user:u_owner', True)
            self.assertEqual(len(rows), 7)
            self.assertTrue({'c_00000', 'c_00001', 'c_00002'}.isdisjoint(chat[0] for chat, _ in rows))

    def test_same_time_total_order_flags_and_nested_settings_match(self):
        path, before, after = self.fixture()
        with sqlite3.connect(path) as connection:
            connection.execute("UPDATE people_members SET joined_at=7")
            connection.execute('''UPDATE people_members SET role='admin',muted=1,silenced=1,
                silenced_until=123, settings='{"nested":["é",true,null,4]}'
                WHERE chat_id='c_00001' AND member='user:u_peer_0' ''')
            connection.commit()
        verify(path, before, after)

    def test_oracle_rejects_batch_query_that_drops_deleted_chat_guard(self):
        path, before, after = self.fixture()
        with sqlite3.connect(path) as connection:
            connection.execute("UPDATE people_chats SET deleted_at=2 WHERE id='c_00001'")
            connection.commit()
        wrong = copy.deepcopy(after)
        wrong['batched_rosters'] = wrong['batched_rosters'].replace('p.deleted_at IS NULL', '1=1')
        with self.assertRaisesRegex(AssertionError, 'outside the selected scope'):
            verify(path, before, wrong)


if __name__ == '__main__':
    unittest.main()

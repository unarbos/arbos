import ctypes as C
import json
from pathlib import Path
import sqlite3
import tempfile
import unittest

from analysis_stats import describe, paired, percentile
from engine import Engine
from fixture import seed
from run import read_position, verify
from source import extract, raw_constant


class StatisticsTests(unittest.TestCase):
    def test_known_percentile_and_interpolation(self):
        self.assertEqual(percentile([40,10,20,30],.5),25)
        self.assertAlmostEqual(describe([10,20,30,40])['p95_ns'],38.5)
    def test_strict_pairing_and_sign_test(self):
        with self.assertRaises(ValueError): paired([1],[1,2])
        with self.assertRaises(ValueError): describe([])
        with self.assertRaises(ValueError): describe([-1])
        r=paired(list(range(11,21)),list(range(1,11)))
        self.assertEqual(r['median_saved_ns'],10)
        self.assertEqual(r['paired_median_saved_95_ci_ns'],[10,10])
        self.assertEqual(r['two_sided_sign_test_p'],2/2**10)
    def test_identical_control_has_no_significance(self):
        r=paired([1,2,3],[1,2,3])
        self.assertEqual(r['two_sided_sign_test_p'],1)
        self.assertEqual(r['non_tied_pairs'],0)


class FixtureTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.source=extract()
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory(prefix='arbos-suite-regression-')
        self.path=Path(self.temp.name)/'fixture.db'
    def tearDown(self): self.temp.cleanup()
    def test_source_queries_match_and_fail_stale_raw_constant(self):
        before=json.loads((Path(__file__).parent/'before-source.json').read_text())
        self.assertEqual(before['room_state'][0],self.source['room_position'])
        for k in ('schema','people_schema','migrations','project_name_index','members_of','chats_of_member'):
            self.assertEqual(before[k],self.source[k])
        with self.assertRaises(ValueError): raw_constant('const CHANGED = "x";', 'SCHEMA')
    def test_relations_compaction_defaults_and_registry_counts(self):
        f=seed(self.path,self.source,'history',1)
        verify(self.path,self.source,self.source,f)
        c=sqlite3.connect(self.path)
        self.assertEqual(c.execute('SELECT COUNT(*) FROM rows r LEFT JOIN devices d ON d.id=r.device WHERE d.id IS NULL').fetchone(),(0,))
        self.assertEqual(c.execute('SELECT COUNT(*) FROM chats c LEFT JOIN projects p ON c.project_id=p.id WHERE p.id IS NULL').fetchone(),(0,))
        self.assertEqual(c.execute("SELECT kind,COUNT(*) FROM people_chats GROUP BY kind ORDER BY kind").fetchall(),[('dm',5),('group',5)])
        self.assertEqual(read_position(c,[self.source['room_position']],'missing'),(0,0))
        with self.assertRaises(sqlite3.IntegrityError): c.execute("INSERT INTO projects(id,name,created_at,last_active_at,user_id) VALUES ('bad','Synthetic project 0',1,1,'u_owner')")
        c.close()
    def test_requires_fresh_path_and_declared_scale(self):
        with self.assertRaises(ValueError): seed(self.path,self.source,'history',2)
        with self.assertRaises(ValueError): seed(self.path,self.source,'production',1)
        self.path.touch()
        with self.assertRaises(ValueError): seed(self.path,self.source,'history',1)
    def test_real_visits_vm_steps_and_n_plus_one(self):
        seed(self.path,self.source,'registry',1)
        e=Engine(self.path)
        try:
            old=e.run(self.source['room_state'],'people/c_00001',profile=True)
            new=e.run([self.source['room_position']],'people/c_00001',profile=True)
            self.assertEqual((old['visits'],new['visits']),(102,1))
            self.assertEqual((old['statements'],new['statements']),(3,1))
            self.assertGreater(old['vm_steps'],new['vm_steps'])
            ctrl=e.run([self.source['chats_of_member'],self.source['members_of']],'user:u_owner',True,True)
            self.assertEqual(ctrl['statements'],11)
            self.assertEqual(ctrl['returned'],60)
            self.assertEqual(ctrl['visits'],70)
            self.assertTrue(any('INDEX' in p for p in e.plan(self.source['room_position'],'people/c_00001')))
        finally: e.close()
    def test_measurement_rejects_writes_multi_statement_and_sql_error(self):
        seed(self.path,self.source,'history',1); e=Engine(self.path)
        try:
            for sql in ('DELETE FROM rows','SELECT 1; DELETE FROM rows','SELECT 1\x00'):
                with self.assertRaises(ValueError): e.run([sql],'')
            with self.assertRaises(RuntimeError): e.run(['SELECT * FROM no_such_table WHERE id=?1'],'synthetic')
            e.lib.sqlite3_exec.argtypes=[C.c_void_p,C.c_char_p,C.c_void_p,C.c_void_p,C.c_void_p]
            self.assertEqual(e.lib.sqlite3_exec(e.db,b'CREATE TABLE must_not_exist(x)',None,None,None),8)
        finally: e.close()


if __name__=='__main__': unittest.main()

"""Build only cached root-lock SQLite, exposing readonly measurements and real NVISIT."""
import ctypes as C
import hashlib
from pathlib import Path
import platform
import re
import subprocess
from source import ROOT

HERE = Path(__file__).resolve().parent
TARGET = HERE / 'target'


def build():
    lock=(ROOT/'Cargo.lock').read_text()
    version=re.search(r'\[\[package\]\]\nname = "libsqlite3-sys"\nversion = "([^"]+)"',lock).group(1)
    candidates=list((Path.home()/'.cargo/registry/src').glob(f'*/libsqlite3-sys-{version}/sqlite3/sqlite3.c'))
    if len(candidates)!=1:
        raise RuntimeError('Exactly one cached root-lock SQLite amalgamation is required; no downloads')
    source=candidates[0]; header=source.with_suffix('.h')
    text=header.read_text(); sqlite_version=re.search(r'#define SQLITE_VERSION\s+"([^"]+)"',text).group(1)
    if 'sqlite3_stmt_scanstatus(' not in text or 'SQLITE_SCANSTAT_NVISIT' not in text:
        raise RuntimeError('Cached SQLite lacks scanstatus API')
    TARGET.mkdir(exist_ok=True)
    policy='-O2 -fPIC -DSQLITE_ENABLE_STMT_SCANSTATUS -DSQLITE_ENABLE_FTS5 -DSQLITE_THREADSAFE=1 '+platform.system()
    key=hashlib.sha256(source.read_bytes()+header.read_bytes()+(HERE/'scan.c').read_bytes()+policy.encode()).hexdigest()
    lib=TARGET/('scan-'+key[:16]+('.dylib' if platform.system()=='Darwin' else '.so'))
    flags=['cc','-O2','-fPIC','-dynamiclib' if platform.system()=='Darwin' else '-shared','-DSQLITE_ENABLE_STMT_SCANSTATUS','-DSQLITE_ENABLE_FTS5','-DSQLITE_THREADSAFE=1','-I',str(source.parent),str(source),str(HERE/'scan.c'),'-o',str(lib)]
    inputs=[source,header,HERE/'scan.c']
    if not lib.exists() or lib.stat().st_mtime < max(p.stat().st_mtime for p in inputs):
        result=subprocess.run(flags,capture_output=True,text=True)
        (TARGET/'build.log').write_text(result.stdout+result.stderr)
        if result.returncode:
            raise RuntimeError(f'Cached SQLite build failed; inspect {TARGET}/build.log')
    if lib.stat().st_size > 100*1024**2:
        raise RuntimeError('Compiler artifact exceeds100MiB budget')
    return lib, {'sqlite_version':sqlite_version,'libsqlite3_sys':version,'sqlite_source_sha256':hashlib.sha256(source.read_bytes()).hexdigest(),'compile_flags':[x for x in flags if not x.startswith('/')], 'library_bytes':lib.stat().st_size,'build_content_sha256':key}


class Result(C.Structure):
    _fields_=[(x,C.c_uint64) for x in ('elapsed_ns','visits','vm_steps','returned','statements')]+[('error',C.c_int)]


class Engine:
    def __init__(self,path):
        lib,self.metadata=build(); self.lib=C.CDLL(str(lib)); self.db=C.c_void_p()
        self.lib.bench_open.argtypes=[C.c_char_p,C.POINTER(C.c_void_p)]
        self.lib.bench_run.argtypes=[C.c_void_p,C.POINTER(C.c_char_p),C.c_int,C.c_char_p,C.c_int,C.c_int,C.POINTER(Result)]
        self.lib.sqlite3_close.argtypes=[C.c_void_p]
        self.lib.sqlite3_libversion.restype=C.c_char_p
        if self.lib.sqlite3_libversion().decode()!=self.metadata['sqlite_version']:
            raise RuntimeError('Loaded SQLite version differs from inspected source')
        rc=self.lib.bench_open(str(path).encode(),C.byref(self.db))
        if rc: raise RuntimeError(f'Readonly SQLite open failed: {rc}')
    def run(self,queries,arg,registry=False,profile=False):
        if not queries or any(not q.lstrip().upper().startswith('SELECT ') or ';' in q or '\x00' in q for q in queries):
            raise ValueError('Only extracted single SELECT queries are allowed')
        values=(C.c_char_p*len(queries))(*(q.encode() for q in queries)); result=Result()
        self.lib.bench_run(self.db,values,len(queries),arg.encode(),registry,profile,C.byref(result))
        if result.error: raise RuntimeError(f'SQLite query failed: {result.error}')
        return {name:getattr(result,name) for name,_ in result._fields_ if name!='error'}
    def close(self):
        if self.db: self.lib.sqlite3_close(self.db); self.db=None

    def plan(self,sql,arg):
        self.lib.sqlite3_prepare_v2.argtypes=[C.c_void_p,C.c_char_p,C.c_int,C.POINTER(C.c_void_p),C.POINTER(C.c_char_p)]
        self.lib.sqlite3_bind_text.argtypes=[C.c_void_p,C.c_int,C.c_char_p,C.c_int,C.c_void_p]
        self.lib.sqlite3_step.argtypes=[C.c_void_p]
        self.lib.sqlite3_column_text.argtypes=[C.c_void_p,C.c_int]
        self.lib.sqlite3_column_text.restype=C.c_char_p
        self.lib.sqlite3_finalize.argtypes=[C.c_void_p]
        st=C.c_void_p(); tail=C.c_char_p()
        rc=self.lib.sqlite3_prepare_v2(self.db,('EXPLAIN QUERY PLAN '+sql).encode(),-1,C.byref(st),C.byref(tail))
        if rc: raise RuntimeError(f'Query plan prepare failed: {rc}')
        try:
            self.lib.sqlite3_bind_text(st,1,arg.encode(),-1,C.c_void_p(-1))
            out=[]
            while (rc:=self.lib.sqlite3_step(st))==100:
                out.append(self.lib.sqlite3_column_text(st,3).decode())
            if rc!=101: raise RuntimeError(f'Query plan failed: {rc}')
            return out
        finally:
            self.lib.sqlite3_finalize(st)

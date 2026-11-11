"""Actual per-file owner transitions over generated files and a fake connection close."""
from pathlib import Path
import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
import time


def repository():
    for root in Path(__file__).resolve().parents:
        if (root/'Cargo.lock').is_file() and (root/'crates/arbos-mac').is_dir(): return root
    raise RuntimeError('Repository root unavailable')


def block(source, signature):
    start=source.index(signature)
    return source[start:source.index('\n}',start)+2]


FIXTURE=r'''
#![allow(dead_code)]
use std::path::{Path,PathBuf};
use std::sync::{Arc,Mutex,Condvar,OnceLock};
use std::sync::atomic::{AtomicUsize,Ordering};
use std::time::Duration;
#[path=WORK_PATH] mod work_lifetime;
#[path=OWNER_PATH] mod store_owner;
use work_lifetime::WorkLifetime;
use store_owner::StoreLease;
static ROOT:OnceLock<Mutex<PathBuf>>=OnceLock::new();
mod credentials {pub fn config_dir()->std::path::PathBuf{super::ROOT.get().unwrap().lock().unwrap().clone()}}
#[derive(Default)]struct GateState{entered:bool,released:bool}
type Gate=Arc<(Mutex<GateState>,Condvar)>;
static NEXT_DROP:OnceLock<Mutex<Option<Gate>>>=OnceLock::new();
mod store {
 use super::*;
 EXACT_PATH_FUNCTIONS
 pub struct Store{path:PathBuf,pause:Option<Gate>}
 impl Store{
  pub fn open(relay:&str,device:&str)->Option<Self>{let path=file_path(relay,device);std::fs::create_dir_all(path.parent()?).ok()?;std::fs::OpenOptions::new().create(true).append(true).open(&path).ok()?;Some(Self{path,pause:NEXT_DROP.get_or_init(Default::default).lock().unwrap().take()})}
  pub fn path(&self)->&Path{&self.path}
  pub fn write(&self,text:&str){std::fs::write(&self.path,text).unwrap()}
 }
 impl Drop for Store{fn drop(&mut self){if let Some(gate)=&self.pause{let(lock,changed)=&**gate;let mut state=lock.lock().unwrap();state.entered=true;changed.notify_all();while !state.released{state=changed.wait(state).unwrap();}}}}
}
struct Scratch(PathBuf);
impl Scratch{
 fn new(label:&str)->Self{static ID:AtomicUsize=AtomicUsize::new(0);loop{let path=std::env::temp_dir().join(format!("arbos-store-owner-{}-{}-{}",std::process::id(),label,ID.fetch_add(1,Ordering::Relaxed)));match std::fs::create_dir(&path){Ok(())=>return Self(path),Err(e)if e.kind()==std::io::ErrorKind::AlreadyExists=>continue,Err(e)=>panic!("create generated fixture: {e}")}}}
 fn select(&self){*ROOT.get_or_init(||Mutex::new(PathBuf::new())).lock().unwrap()=self.0.clone();}
}
impl Drop for Scratch{fn drop(&mut self){let _=std::fs::remove_dir_all(&self.0);}}
fn ticket()->work_lifetime::WorkTicket{WorkLifetime::default().ticket().unwrap()}
fn release(gate:&Gate){let(lock,changed)=&**gate;lock.lock().unwrap().released=true;changed.notify_all();}
fn entered(gate:&Gate)->bool{let(lock,changed)=&**gate;let state=lock.lock().unwrap();let(state,_)=changed.wait_timeout_while(state,Duration::from_secs(1),|state|!state.entered).unwrap();state.entered}
#[cfg(test)]mod store_owner_checks{
 use super::*;
 #[test]fn late_receipt_cannot_unlink_reincarnated_owner(){
  let root=Scratch::new("late");root.select();let a=StoreLease::new("http://same.invalid","device",None,ticket());assert!(a.open());a.with(|s|s.write("A"));let old=a.retire(true).unwrap();assert!(!a.is_open());let b=StoreLease::new("http://same.invalid","device",None,ticket());assert!(b.open());b.with(|s|s.write("B"));old.finish();assert_eq!(std::fs::read_to_string(store::file_path("http://same.invalid","device")).unwrap(),"B");assert!(a.with(|_|()).is_none());assert!(a.retire(true).is_none());assert!(b.is_open());
 }
 #[test]fn same_file_opener_waits_through_close_and_unlink(){
  let root=Scratch::new("barrier");root.select();let gate=Arc::new((Mutex::new(GateState::default()),Condvar::new()));*NEXT_DROP.get_or_init(Default::default).lock().unwrap()=Some(gate.clone());let a=StoreLease::new("http://same.invalid","device",None,ticket());assert!(a.open());let old=a.retire(true).unwrap();let closer=std::thread::spawn(move||old.finish());let reached=entered(&gate);let b=Arc::new(StoreLease::new("http://same.invalid","device",None,ticket()));let(next,done)=std::sync::mpsc::channel();let(start,started)=std::sync::mpsc::channel();let next_owner=b.clone();let opener=std::thread::spawn(move||{start.send(()).unwrap();let opened=next_owner.open();if opened{next_owner.with(|s|s.write("B"));}next.send(opened).unwrap();});let began=started.recv_timeout(Duration::from_secs(1)).is_ok();let overran=done.recv_timeout(Duration::from_millis(30)).ok();release(&gate);closer.join().unwrap();opener.join().unwrap();assert!(reached&&began);assert!(overran.is_none(),"B opened before old close/unlink completed");assert!(done.recv_timeout(Duration::from_secs(1)).unwrap());assert_eq!(std::fs::read_to_string(store::file_path("http://same.invalid","device")).unwrap(),"B");
 }
 #[test]fn different_file_is_not_blocked_by_old_connection_close(){
  let root=Scratch::new("independent");root.select();let gate=Arc::new((Mutex::new(GateState::default()),Condvar::new()));*NEXT_DROP.get_or_init(Default::default).lock().unwrap()=Some(gate.clone());let a=StoreLease::new("http://same.invalid","old",None,ticket());assert!(a.open());let old=a.retire(true).unwrap();let closer=std::thread::spawn(move||old.finish());let reached=entered(&gate);let b=StoreLease::new("http://same.invalid","other",None,ticket());let opened=b.open();release(&gate);closer.join().unwrap();assert!(reached&&opened);
 }
 #[test]fn duplicate_receipt_does_not_revoke_new_owner(){
  let root=Scratch::new("duplicate");root.select();let a=StoreLease::new("http://same.invalid","device",None,ticket());assert!(a.open());let first=a.retire(false).unwrap();let second=a.retire(true).unwrap();first.finish();let b=StoreLease::new("http://same.invalid","device",None,ticket());assert!(b.open());b.with(|s|s.write("B"));second.finish();assert!(b.is_open());assert_eq!(std::fs::read_to_string(store::file_path("http://same.invalid","device")).unwrap(),"B");
 }
 #[test]fn active_file_cannot_be_taken_by_another_live_owner(){
  let root=Scratch::new("live");root.select();let a=StoreLease::new("http://same.invalid","device",None,ticket());assert!(a.open());a.with(|s|s.write("A"));let b=StoreLease::new("http://same.invalid","device",None,ticket());assert!(!b.open());assert!(b.with(|_|()).is_none());assert!(b.retire(true).is_none());assert_eq!(std::fs::read_to_string(store::file_path("http://same.invalid","device")).unwrap(),"A");assert!(a.open());
 }
 #[test]fn actual_case_alias_observation_and_exact_origin_refusal(){
  let root=Scratch::new("case");root.select();let upper=store::file_path("http://case.invalid","DeviceA");let lower=store::file_path("http://case.invalid","devicea");let a=StoreLease::new("http://case.invalid","DeviceA",None,ticket());assert!(a.open());a.with(|s|s.write("A"));let case_insensitive=lower.exists();println!("CASE_INSENSITIVE={case_insensitive}");let b=StoreLease::new("http://case.invalid","devicea",None,ticket());assert!(!b.open());assert!(b.with(|_|()).is_none());assert_eq!(std::fs::read_to_string(&upper).unwrap(),"A");
 }
 #[test]fn punctuation_collision_does_not_prove_shared_origin(){
  let root=Scratch::new("punctuation");root.select();let left="http://a-b.invalid";let right="http://a.b.invalid";assert_eq!(store::file_path(left,"device"),store::file_path(right,"device"));let a=StoreLease::new(left,"device",None,ticket());assert!(a.open());a.with(|s|s.write("A"));let b=StoreLease::new(right,"device",None,ticket());assert!(!b.open());assert!(b.with(|_|()).is_none());assert_eq!(std::fs::read_to_string(store::file_path(left,"device")).unwrap(),"A");
 }
 #[test]fn sequential_collision_still_refuses_old_cached_origin(){
  let root=Scratch::new("sequential");root.select();let left="http://a-b.invalid";let right="http://a.b.invalid";let a=StoreLease::new(left,"device",None,ticket());assert!(a.open());a.with(|s|s.write("A"));a.retire(false).unwrap().finish();drop(a);let b=StoreLease::new(right,"device",None,ticket());assert!(!b.open(),"dropping all old handles must not authorize a colliding origin");assert!(b.with(|_|()).is_none());assert_eq!(std::fs::read_to_string(store::file_path(left,"device")).unwrap(),"A");
 }
 #[test]fn known_user_change_refuses_prior_user_cache_without_wipe(){
  let root=Scratch::new("users");root.select();let a=StoreLease::new("http://same.invalid","device",Some("user:A"),ticket());assert!(a.open());a.with(|s|s.write("A"));a.retire(false).unwrap().finish();drop(a);let b=StoreLease::new("http://same.invalid","device",Some("user:B"),ticket());assert!(!b.open());assert!(b.with(|_|()).is_none());assert!(b.retire(true).is_none());assert_eq!(std::fs::read_to_string(store::file_path("http://same.invalid","device")).unwrap(),"A");let again=StoreLease::new("http://same.invalid","device",Some("user:A"),ticket());assert!(again.open());
 }
 #[test]fn discovering_known_user_is_retained_after_unknown_lease(){
  let root=Scratch::new("discovery");root.select();let a=StoreLease::new("http://same.invalid","device",None,ticket());assert!(a.open());a.retire(false).unwrap().finish();drop(a);let known=StoreLease::new("http://same.invalid","device",Some("user:A"),ticket());assert!(known.open());known.retire(false).unwrap().finish();drop(known);let b=StoreLease::new("http://same.invalid","device",Some("user:B"),ticket());assert!(!b.open());
 }
 #[test]fn existing_parent_alias_keeps_one_file_owner(){
  let root=Scratch::new("parent");let real=root.0.join("real");std::fs::create_dir(&real).unwrap();let alias=root.0.join("alias");
  #[cfg(unix)]std::os::unix::fs::symlink(&real,&alias).unwrap();
  #[cfg(not(unix))]return;
  *ROOT.get_or_init(||Mutex::new(PathBuf::new())).lock().unwrap()=real.clone();let a=StoreLease::new("http://same.invalid","device",None,ticket());assert!(a.open());let old=a.retire(true).unwrap();*ROOT.get().unwrap().lock().unwrap()=alias;let b=StoreLease::new("http://same.invalid","device",None,ticket());assert!(b.open());b.with(|s|s.write("B"));old.finish();assert_eq!(std::fs::read_to_string(store::file_path("http://same.invalid","device")).unwrap(),"B");
 }
}
'''


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tree',type=Path,default=repository());parser.add_argument('--record-dir',type=Path)
    args=parser.parse_args();root=args.tree.resolve()
    owner=root/'crates/arbos-mac/src/sync/store_owner.rs';work=root/'crates/arbos-mac/src/sync/work_lifetime.rs';store=root/'crates/arbos-mac/src/sync/store.rs'
    paths=[owner,work,store];before={str(p.relative_to(root)):hashlib.sha256(p.read_bytes()).hexdigest() for p in paths}
    source=store.read_text();exact='\n'.join(block(source,sig) for sig in ['fn slug(', 'pub fn file_path(', 'pub fn remove_files('])
    fixture=FIXTURE.replace('WORK_PATH',json.dumps(str(work))).replace('OWNER_PATH',json.dumps(str(owner))).replace('EXACT_PATH_FUNCTIONS',exact)
    start=time.monotonic()
    with tempfile.TemporaryDirectory(prefix='arbos-store-owner-build-') as directory:
        temp=Path(directory);(temp/'fixture.rs').write_text(fixture);binary=temp/'store-owner-check'
        env={'PATH':os.environ['PATH'],'TMPDIR':str(temp)}
        fence=['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)(deny file-read* (subpath "/Users/Shared"))'] if sys.platform=='darwin' else []
        build=subprocess.run(fence+['rustc','--edition=2021','--test',str(temp/'fixture.rs'),'-o',str(binary)],env=env,text=True,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,timeout=30)
        if build.returncode:code=build.returncode;output=build.stdout
        else:
            run=subprocess.run(fence+[str(binary),'store_owner_checks::','--test-threads=1','--nocapture'],env=env,text=True,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,timeout=15);code=run.returncode;output=run.stdout
        after={str(p.relative_to(root)):hashlib.sha256(p.read_bytes()).hexdigest() for p in paths}
        aliases=re.findall(r'CASE_INSENSITIVE=(true|false)',output)
        row={'exit_code':code,'compile_exit':build.returncode,'seconds':time.monotonic()-start,'source_sha256':before,'source_unchanged':before==after,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'case_insensitive_filesystem':aliases[0]=='true' if len(aliases)==1 else None,'tests':11,'output':output}
        if args.record_dir:
            args.record_dir.mkdir(parents=True,exist_ok=True)
            (args.record_dir/'store-owner.json').write_text(json.dumps(row,indent=2)+'\n');(args.record_dir/'store-owner.log').write_text(output);(args.record_dir/'store-owner-fixture.rs').write_text(fixture)
        print(json.dumps({k:v for k,v in row.items() if k!='output'}))
        if code:print(output[-5000:])
        return 0 if code==0 and before==after and '11 passed; 0 failed' in output else 1

if __name__=='__main__':raise SystemExit(main())

"""Actual call retry helper over synthetic HTTP/refresh futures."""
from pathlib import Path
import argparse,hashlib,json,os,subprocess,tempfile,time
ROOT=Path(__file__).resolve().parent
from support import repository, dependency_version, fence, record
REPO=repository(ROOT);SOURCE=Path('crates/arbos-mac/src/sync/relay.rs')
PRELUDE=r'''
use std::sync::{Arc,Mutex,atomic::{AtomicBool,Ordering}};use serde_json::Value;use anyhow::{Result,Context as _};use tokio::sync::Notify;
type WorkTicket=u64;struct CallAuthority{owner:WorkTicket,CAP_USER_FIELD bearer:String}
mod reqwest{#[derive(Clone)]pub enum Method{POST}#[derive(Clone,Copy)]pub struct StatusCode(pub u16);impl StatusCode{pub fn is_success(&self)->bool{self.0<400}}}
#[derive(Debug)]struct RelayError;impl RelayError{fn from_body(_:reqwest::StatusCode,_:&str)->Self{Self}fn is_token_expired(&self)->bool{true}}impl std::fmt::Display for RelayError{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{f.write_str("synthetic")}}impl std::error::Error for RelayError{}
struct State{owner:u64,token:String,headers:Vec<String>}
struct RefreshGate{began:AtomicBool,release:Notify}
#[derive(Clone)]struct Http{state:Arc<Mutex<State>>}
struct Request{state:Arc<Mutex<State>>,token:String}
impl Http{fn post(&self,_:String)->Request{Request{state:self.state.clone(),token:String::new()}}fn request(&self,_:reqwest::Method,_:String)->Request{self.post(String::new())}}
impl Request{fn bearer_auth(mut self,t:&str)->Self{self.token=t.into();self}fn json(self,_:&Value)->Self{self}fn header(self,_:&str,_:&String)->Self{self}async fn send(self)->Result<Response>{let mut s=self.state.lock().unwrap();s.headers.push(self.token);Ok(Response(if s.headers.len()==1{401}else{200}))}}
struct Response(u16);impl Response{fn status(&self)->reqwest::StatusCode{reqwest::StatusCode(self.0)}async fn text(self)->Result<String>{Ok("{}".into())}}
struct Shared{http:Http,http_url:String,state:Arc<Mutex<State>>,gate:Option<Arc<RefreshGate>>}
impl Shared{
 fn token(&self)->String{self.state.lock().unwrap().token.clone()}fn token_for_owner(&self,o:u64)->Result<String>{let s=self.state.lock().unwrap();anyhow::ensure!(s.owner==o,"retired account");Ok(s.token.clone())}fn relay_answered(&self){}
 async fn refresh_tokens(&self,_:Option<&str> EXPECTED_ARG)->Result<()>{CHECK_OWNER_BEFORE if let Some(g)=&self.gate{g.began.store(true,Ordering::SeqCst);g.release.notified().await;}CHECK_OWNER_AFTER let mut s=self.state.lock().unwrap();if s.owner==1{s.token="A2".into();}Ok(())}
'''
TESTS=r'''
}
fn setup(paused:bool)->Arc<Shared>{let state=Arc::new(Mutex::new(State{owner:1,token:"A".into(),headers:vec![]}));Arc::new(Shared{http:Http{state:state.clone()},http_url:"http://synthetic.invalid".into(),state,gate:paused.then(||Arc::new(RefreshGate{began:AtomicBool::new(false),release:Notify::new()}))})}
async fn invoke(s:&Arc<Shared>)->bool{invoke_bearer(s).await.is_ok()}
async fn invoke_bearer(s:&Arc<Shared>)->Result<String>{let result=INVOKE?;BEARER_RESULT}
#[tokio::main(flavor="current_thread")]async fn main(){let mut failed=0;let s=setup(false);let ok=invoke(&s).await&&s.state.lock().unwrap().headers==["A","A2"];println!("{} same-account refresh control",if ok{"PASS"}else{"FAIL"});if !ok{failed+=1}
let s=setup(true);let c=s.clone();let task=tokio::spawn(async move{invoke(&c).await});let g=s.gate.as_ref().unwrap();for _ in 0..1000{if g.began.load(Ordering::SeqCst){break}tokio::task::yield_now().await;} {let mut state=s.state.lock().unwrap();state.owner=2;state.token="B".into();}g.release.notify_one();let _=task.await;let ok=s.state.lock().unwrap().headers==["A"];println!("{} stale retry never uses replacement bearer",if ok{"PASS"}else{"FAIL"});if !ok{failed+=1}let s=setup(false);{let mut state=s.state.lock().unwrap();state.owner=2;state.token="B".into();}let _=invoke(&s).await;let ok=s.state.lock().unwrap().headers.is_empty();println!("{} stale owner rejected before initial request",if ok{"PASS"}else{"FAIL"});if !ok{failed+=1}
let s=setup(false);let used=invoke_bearer(&s).await.unwrap_or_default();{let mut state=s.state.lock().unwrap();state.owner=2;state.token="B".into();}let ok=used=="A2";println!("{} cleanup retains refreshed original request authority",if ok{"PASS"}else{"FAIL"});if !ok{failed+=1}
println!("RESULT tests=4 failed={failed}");std::process::exit(if failed==0{0}else{1})}
'''
def main():
 p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,default=REPO);p.add_argument('--label',default='owned-http');p.add_argument('--record-dir',type=Path);args=p.parse_args();s=(args.tree/SOURCE).read_text();after='async fn call_json_status_owned('in s;name='call_json_status_owned'if after else'post_json_status';a=s.index('    async fn '+name+'(');b=s.index('\n    }',a)+6;method=s[a:b]
 if '    async fn call_json_status_admitted(' in s:
  a=s.index('    async fn call_json_status_admitted(');method+='\n'+s[a:s.index('\n    }',a)+6]
 fixture=PRELUDE+method+TESTS
 replacements={'EXPECTED_ARG':',expected:Option<u64>'if after else'','CHECK_OWNER_BEFORE':'if let Some(owner)=expected{self.token_for_owner(owner)?;}'if after else'','CHECK_OWNER_AFTER':'if let Some(owner)=expected{self.token_for_owner(owner)?;}'if after else'','INVOKE':'s.call_json_status_owned(CallAuthority{owner:1,CAP_USER_VALUE bearer:"A".into()},reqwest::Method::POST,"/calls/old/leave",Some(Value::Null),&[]).await'if after else's.post_json_status("/calls/old/leave",Value::Null,&[]).await','BEARER_RESULT':'Ok(result.2.bearer)'if after else'Ok("A".into())'}
 for a,b in replacements.items():fixture=fixture.replace(a,b)
 fixture=fixture.replace('CAP_USER_FIELD','user_id:Option<String>,' if 'user_id: Option<String>' in s else '').replace('CAP_USER_VALUE','user_id:Some("A".into()),' if 'user_id: Option<String>' in s else '')
 start=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='arbos-call-http-')as temp:
  temp=Path(temp);(temp/'main.rs').write_text(fixture);(temp/'Cargo.toml').write_text('[workspace]\n[package]\nname="call-http-check"\nversion="0.0.0"\nedition="2021"\n[[bin]]\nname="http-check"\npath="main.rs"\n[dependencies]\ntokio={version="=LOCKED_TOKIO",features=["sync","rt","macros"]}\nserde_json="1"\nanyhow="1"\n'.replace('LOCKED_TOKIO',dependency_version(REPO,'tokio')))
  env={'PATH':os.environ['PATH'],'HOME':str(temp),'TMPDIR':str(temp),'CARGO_HOME':str(Path.home()/'.cargo'),'RUSTUP_HOME':str(Path.home()/'.rustup'),'CARGO_NET_OFFLINE':'true','CARGO_TARGET_DIR':str(temp/'target')}
  compile=subprocess.run(fence(['cargo','build','--offline','--manifest-path',str(temp/'Cargo.toml')]),env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
  if compile.returncode:output=compile.stdout;code=compile.returncode
  else:
   p=subprocess.run(fence([str(temp/'target/debug/http-check')]),env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True,timeout=10);output=p.stdout;code=p.returncode
 result={'exit_code':code,'compile_exit':compile.returncode,'wall_seconds':time.monotonic()-start,'source_sha256':hashlib.sha256(s.encode()).hexdigest(),'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'output':output}
 record(args.record_dir,args.label,result,output);print(json.dumps(result));return code
if __name__=='__main__':raise SystemExit(main())

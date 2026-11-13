"""Actual relay call command arms, synthetic response futures and no network."""
from pathlib import Path
import argparse,hashlib,json,os,subprocess,tempfile,time
ROOT=Path(__file__).resolve().parent
from support import repository, dependency_version, fence, record
REPO=repository(ROOT)
SOURCE=Path('crates/arbos-mac/src/sync/relay.rs')
PRELUDE=r'''
use std::collections::{HashMap,HashSet};use std::sync::{Arc,Mutex,RwLock,atomic::{AtomicBool,Ordering}};use tokio::sync::{mpsc,Notify};use serde::{Serialize,Deserialize};use serde_json::{json,Value};use anyhow::Result;
#[path=ATTEMPT_PATH]mod call_attempt;use call_attempt::CallAttempt;
#[derive(Clone,Debug,Serialize)]enum CallKind{Voice}
#[derive(Default,Clone,Debug,Deserialize)]struct Call{call_id:String,chat_id:String}impl From<&Call>for Call{fn from(v:&Call)->Self{v.clone()}}
#[derive(Clone,Debug,Deserialize)]struct Reachable;impl From<&Reachable>for Reachable{fn from(v:&Reachable)->Self{v.clone()}}
#[derive(Deserialize)]struct StartCallResponseDto{call:Call,joined:bool,reachable:Option<Vec<Reachable>>}
#[derive(Deserialize)]struct LeaveCallResponseDto{ended:bool,duration_s:i64}
#[derive(Default)]struct CallLeft{ended:bool,duration_s:u64}
struct CallEnded{call_id:String,chat_id:String,project_id:String,duration_s:u64,reason:String,writer_device:Option<String>,writer_is_me:bool}
struct UserError;impl UserError{fn from_text(_: &str,_:&str)->Self{Self}}
fn call_error(_:&str,_:&anyhow::Error)->UserError{UserError}
fn parse_call(v:&Value)->Option<Call>{serde_json::from_value(v.get("call").unwrap_or(v).clone()).ok()}
fn project_of(_:&Shared,_:&str)->Option<String>{None}fn presence_now(_:&Shared,_:&str){}
mod reqwest{pub enum Method{POST,GET}}
mod update{pub fn client_header()->String{"synthetic".into()}}
enum RoomCmd{Presence}struct Room{cmds:mpsc::UnboundedSender<RoomCmd>}
enum ClientCommand{StartCall{CMD_ATTEMPT chat_id:String,kind:CallKind},JoinCall{CMD_ATTEMPT call_id:String},LeaveCall{CMD_ATTEMPT call_id:String}}
enum SyncEvent{CallReady{CMD_ATTEMPT call:Call,created:bool,reachable:Vec<Reachable>},CallError{CMD_ATTEMPT chat_id:String,error:UserError},CallInfo(Call),CallEnded(CallEnded),CallLeft{attempt:CallAttempt,ended:CallEnded},CallLeftInfo{attempt:CallAttempt,call:Call},CallLeaveError{attempt:CallAttempt,error:UserError}}
struct Gate{began:AtomicBool,release:Notify,value:Value}impl Gate{fn new(value:Value)->Arc<Self>{Arc::new(Self{began:AtomicBool::new(false),release:Notify::new(),value})}}
struct Http{log:Arc<Mutex<Vec<(String,String)>>>,gates:Arc<Mutex<HashMap<String,Arc<Gate>>>>}
impl Http{fn post(&self,url:String)->Request{Request{http:Self{log:self.log.clone(),gates:self.gates.clone()},path:url.trim_start_matches("http://synthetic.invalid").into(),token:String::new()}}}
struct Request{http:Http,path:String,token:String}impl Request{fn bearer_auth(mut self,t:impl AsRef<str>)->Self{self.token=t.as_ref().into();self}fn json(self,_:&Value)->Self{self}async fn send(self)->Result<()>{self.http.log.lock().unwrap().push((self.path.clone(),self.token));let gate=self.http.gates.lock().unwrap().get(&self.path).cloned();if let Some(g)=gate{g.began.store(true,Ordering::SeqCst);g.release.notified().await;}Ok(())}}
#[derive(Clone)]struct User{id:String}struct Credentials{access_token:String,user:Option<User>}
struct Shared{account_lifetime:Mutex<WorkLifetime>,creds:RwLock<Option<Credentials>>,http:Http,http_url:String,events:mpsc::UnboundedSender<SyncEvent>,in_call:Mutex<Option<String>>,call_attempt:Mutex<Option<CallAttempt>>,call_requests:tokio::sync::Mutex<()>,rooms:Mutex<HashMap<String,Room>>,requests:Mutex<Vec<String>>,gates:Arc<Mutex<HashMap<String,Arc<Gate>>>>}
impl Shared{
 fn me_user(&self)->Option<User>{self.creds.read().unwrap().as_ref().and_then(|c|c.user.clone())}
 fn token(&self)->String{self.creds.read().unwrap().as_ref().map(|c|c.access_token.clone()).unwrap_or_default()}fn device_id(&self)->String{"original-device".into()}fn is_people(&self,_:&str)->bool{true}fn fail(&self,_:&str,_:&anyhow::Error){}
 async fn post_json_status(&self,p:&str,_:Value,_:&[(&str,String)])->Result<((),Value)>{let value=self.request(p).await;anyhow::ensure!(value.get("fixture_reject").and_then(Value::as_bool)!=Some(true),"synthetic request refused");Ok(((),value))}
 async fn call_json_status_owned(&self,authority:CallAuthority,_:reqwest::Method,p:&str,v:Option<Value>,e:&[(&str,String)])->Result<((),Value,CallAuthority)>{self.token_for_owner(authority.owner)?;let (_,v)=self.post_json_status(p,v.unwrap_or(Value::Null),e).await?;Ok(((),v,authority))}fn token_for_owner(&self,o:WorkTicket)->Result<String>{anyhow::ensure!(self.account_lifetime.lock().unwrap().accepts(o),"retired");Ok(self.token())}

 async fn post_json(&self,p:&str,_:Value)->Result<Value>{Ok(self.request(p).await)}async fn get_json(&self,_:&str)->Result<Value>{Ok(Value::Null)}
 async fn request(&self,p:&str)->Value{self.requests.lock().unwrap().push(p.into());let gate=self.gates.lock().unwrap().get(p).cloned();if let Some(g)=gate{g.began.store(true,Ordering::SeqCst);g.release.notified().await;g.value.clone()}else{json!({"ended":true,"duration_s":1})}}
}
fn setup()->(Arc<Shared>,mpsc::UnboundedReceiver<SyncEvent>){let(tx,rx)=mpsc::unbounded_channel();let gates=Arc::new(Mutex::new(HashMap::new()));let s=Arc::new(Shared{account_lifetime:Mutex::new(WorkLifetime::default()),creds:RwLock::new(Some(Credentials{access_token:"original-authority".into(),user:Some(User{id:"A".into()})})),http:Http{log:Arc::default(),gates:gates.clone()},http_url:"http://synthetic.invalid".into(),events:tx,in_call:Mutex::new(None),call_attempt:Mutex::new(None),call_requests:tokio::sync::Mutex::new(()),rooms:Mutex::new(HashMap::new()),requests:Mutex::new(vec![]),gates});(s,rx)}
fn command(shared:&Arc<Shared>,command:ClientCommand){match command{
'''
TESTS=r'''
}}
async fn spin(f:impl Fn()->bool)->bool{for _ in 0..1000{if f(){return true}tokio::task::yield_now().await;}f()}
fn start(a:&CallAttempt,chat:&str)->ClientCommand{ClientCommand::StartCall{PASS_ATTEMPT chat_id:chat.into(),kind:CallKind::Voice}}
fn join(a:&CallAttempt,id:&str)->ClientCommand{ClientCommand::JoinCall{PASS_ATTEMPT call_id:id.into()}}
fn gate(s:&Shared,p:&str,v:Value)->Arc<Gate>{let g=Gate::new(v);s.gates.lock().unwrap().insert(p.into(),g.clone());g}
fn answer(id:&str,chat:&str)->Value{json!({"call":{"call_id":id,"chat_id":chat},"joined":false})}
async fn late_response()->bool{let(s,mut rx)=setup();let g=gate(&s,"/chats/old/calls",answer("old-call","old"));let a=CallAttempt::new("old".into(),None);command(&s,start(&a,"old"));if !spin(||g.began.load(Ordering::SeqCst)).await{return false}a.cancel();g.release.notify_one();for _ in 0..100{tokio::task::yield_now().await;}let ready=matches!(rx.try_recv(),Ok(SyncEvent::CallReady{..}));!ready&&s.http.log.lock().unwrap().iter().any(|(p,t)|p=="/calls/old-call/leave"&&t=="original-authority")&&s.in_call.lock().unwrap().is_none()}
async fn same_call_variant(reopen:bool,different:bool)->bool{let(s,mut rx)=setup();let g=gate(&s,"/chats/same/calls",answer("same-call","same"));let j=gate(&s,"/calls/same-call/join",json!({"call":{"call_id":"same-call","chat_id":"same"}}));let a=CallAttempt::new("same".into(),None);command(&s,start(&a,"same"));if !spin(||g.began.load(Ordering::SeqCst)).await{return false}a.cancel();if reopen{let mut life=s.account_lifetime.lock().unwrap();life.close();life.reopen();}if different{s.creds.write().unwrap().as_mut().unwrap().user=Some(User{id:"B".into()});}let b=CallAttempt::new("same".into(),Some("same-call".into()));command(&s,join(&b,"same-call"));g.release.notify_one();if !spin(||j.began.load(Ordering::SeqCst)).await{return false}j.release.notify_one();for _ in 0..100{tokio::task::yield_now().await;}let mut ready=0;while let Ok(e)=rx.try_recv(){if matches!(e,SyncEvent::CallReady{..}){ready+=1}}ready==1&&(if different{s.http.log.lock().unwrap().iter().any(|(p,t)|p=="/calls/same-call/leave"&&t=="original-authority")}else{s.http.log.lock().unwrap().is_empty()})&&s.in_call.lock().unwrap().as_deref()==Some("same-call")}
async fn serial_cleanup()->bool{let(s,_)=setup();let g=gate(&s,"/chats/old/calls",answer("old-call","old"));let close=gate(&s,"/calls/old-call/leave",Value::Null);let new=gate(&s,"/chats/new/calls",answer("new-call","new"));let a=CallAttempt::new("old".into(),None);command(&s,start(&a,"old"));if !spin(||g.began.load(Ordering::SeqCst)).await{return false}a.cancel();g.release.notify_one();if !spin(||close.began.load(Ordering::SeqCst)).await{return false}let b=CallAttempt::new("new".into(),None);command(&s,start(&b,"new"));for _ in 0..100{tokio::task::yield_now().await;}let held=!new.began.load(Ordering::SeqCst);close.release.notify_one();let resumed=spin(||new.began.load(Ordering::SeqCst)).await;new.release.notify_one();held&&resumed}
async fn ordinary()->bool{let(s,mut rx)=setup();let g=gate(&s,"/chats/ok/calls",answer("ok-call","ok"));let a=CallAttempt::new("ok".into(),None);command(&s,start(&a,"ok"));if !spin(||g.began.load(Ordering::SeqCst)).await{return false}g.release.notify_one();for _ in 0..100{tokio::task::yield_now().await;}matches!(rx.try_recv(),Ok(SyncEvent::CallReady{call,..})if call.call_id=="ok-call")&&s.in_call.lock().unwrap().as_deref()==Some("ok-call")}
async fn queued_commands()->bool{let(s,_)=setup();let a=CallAttempt::new("old".into(),Some("old-call".into()));*s.call_attempt.lock().unwrap()=Some(a.clone());{let mut life=s.account_lifetime.lock().unwrap();life.close();life.reopen();}s.creds.write().unwrap().as_mut().unwrap().access_token="replacement-authority".into();command(&s,start(&a,"old"));command(&s,join(&a,"old-call"));command(&s,ClientCommand::LeaveCall{PASS_ATTEMPT call_id:"old-call".into()});for _ in 0..100{tokio::task::yield_now().await;}s.requests.lock().unwrap().is_empty()&&s.http.log.lock().unwrap().is_empty()}
async fn valid_leave()->bool{let(s,mut rx)=setup();let g=gate(&s,"/chats/ok/calls",answer("ok-call","ok"));let a=CallAttempt::new("ok".into(),None);command(&s,start(&a,"ok"));if !spin(||g.began.load(Ordering::SeqCst)).await{return false}g.release.notify_one();for _ in 0..100{tokio::task::yield_now().await;}while rx.try_recv().is_ok(){}a.cancel();command(&s,ClientCommand::LeaveCall{PASS_ATTEMPT call_id:"ok-call".into()});for _ in 0..100{tokio::task::yield_now().await;}s.requests.lock().unwrap().contains(&"/calls/ok-call/leave".into())&&s.in_call.lock().unwrap().is_none()}
async fn preserved_then_failed()->bool{
 let(s,mut rx)=setup();let old=gate(&s,"/chats/same/calls",answer("same-call","same"));let rejected=gate(&s,"/calls/same-call/join",json!({"fixture_reject":true}));
 let a=CallAttempt::new("same".into(),None);command(&s,start(&a,"same"));if !spin(||old.began.load(Ordering::SeqCst)).await{return false}
 let b=attempt_for(&s,"same".into(),Some("same-call".into()));command(&s,join(&b,"same-call"));old.release.notify_one();if !spin(||rejected.began.load(Ordering::SeqCst)).await{return false}
 rejected.release.notify_one();for _ in 0..100{tokio::task::yield_now().await;}
 let mut failed=false;while let Ok(event)=rx.try_recv(){if matches!(event,SyncEvent::CallError{..}){failed=true;b.cancel();}}
 for _ in 0..100{tokio::task::yield_now().await;}
 failed&&s.http.log.lock().unwrap().iter().any(|(path,bearer)|path=="/calls/same-call/leave"&&bearer=="original-authority")
}
async fn invalid_join(empty:bool)->bool{
 let(s,mut rx)=setup();let value=if empty{json!({"call":{"call_id":"","chat_id":"same"}})}else{json!({})};let response=gate(&s,"/calls/bad/join",value);let a=CallAttempt::new("same".into(),Some("bad".into()));command(&s,join(&a,"bad"));if !spin(||response.began.load(Ordering::SeqCst)).await{return false}response.release.notify_one();for _ in 0..100{tokio::task::yield_now().await;}
 let(mut failed,mut ready)=(false,false);while let Ok(event)=rx.try_recv(){failed|=matches!(event,SyncEvent::CallError{..});ready|=matches!(event,SyncEvent::CallReady{..});}failed&&!ready
}
fn capture_retirement(s:&Shared)->Vec<RetiredCall>{
 let mut life=s.account_lifetime.lock().unwrap();let captured=CAPTURE_RETIRE;
 life.close();*s.creds.write().unwrap()=None;captured
}
async fn replacement_chain(final_ok:bool)->bool{
 let(s,mut rx)=setup();let old=gate(&s,"/chats/same/calls",answer("same-call","same"));let b_reply=gate(&s,"/calls/same-call/join",json!({"fixture_reject":true}));
 let a=attempt_for(&s,"same".into(),None);command(&s,start(&a,"same"));if !spin(||old.began.load(Ordering::SeqCst)).await{return false}
 let b=attempt_for(&s,"same".into(),Some("same-call".into()));command(&s,join(&b,"same-call"));old.release.notify_one();if !spin(||b_reply.began.load(Ordering::SeqCst)).await{return false}
 let value=if final_ok{json!({"call":{"call_id":"same-call","chat_id":"same"}})}else{json!({"fixture_reject":true})};let c_reply=gate(&s,"/calls/same-call/join",value);
 let c=attempt_for(&s,"same".into(),Some("same-call".into()));command(&s,join(&c,"same-call"));b_reply.release.notify_one();if !spin(||c_reply.began.load(Ordering::SeqCst)).await{return false}c_reply.release.notify_one();for _ in 0..100{tokio::task::yield_now().await;}
 let(mut ready,mut errors)=(0,0);while let Ok(event)=rx.try_recv(){match event{SyncEvent::CallReady{attempt,..}if attempt.is_same(&c)=>ready+=1,SyncEvent::CallError{..}=>errors+=1,_=>{}}}
 let leaves=s.http.log.lock().unwrap().iter().filter(|(path,_)|path=="/calls/same-call/leave").count();if final_ok{ready==1&&errors==0&&leaves==0}else{ready==0&&errors==1&&leaves==1}
}
async fn inherited_retirement(same_user:bool)->bool{
 let(s,_)=setup();let old=gate(&s,"/chats/same/calls",answer("same-call","same"));let b_reply=gate(&s,"/calls/same-call/join",json!({"fixture_reject":true}));
 let a=attempt_for(&s,"same".into(),None);command(&s,start(&a,"same"));if !spin(||old.began.load(Ordering::SeqCst)).await{return false}
 let b=attempt_for(&s,"same".into(),Some("same-call".into()));command(&s,join(&b,"same-call"));old.release.notify_one();if !spin(||b_reply.began.load(Ordering::SeqCst)).await{return false}
 let retired=capture_retirement(&s);if retired.len()!=1{return false}
 let(mut fresh,mut rx)=setup();Arc::get_mut(&mut fresh).unwrap().call_registry=s.call_registry.clone();if !same_user{fresh.creds.write().unwrap().as_mut().unwrap().user=Some(User{id:"B".into()});}
 let c_reply=gate(&fresh,"/calls/same-call/join",json!({"call":{"call_id":"same-call","chat_id":"same"}}));let c=attempt_for(&fresh,"same".into(),Some("same-call".into()));command(&fresh,join(&c,"same-call"));b_reply.release.notify_one();if !spin(||c_reply.began.load(Ordering::SeqCst)).await{return false}
 // Actual capture helper has already closed A; replay its bounded effect with its captured authority.
 for captured in retired{compensate_call_join(&s,&captured.call,&captured.authority).await;}
 c_reply.release.notify_one();for _ in 0..100{tokio::task::yield_now().await;}
 let ready=matches!(rx.try_recv(),Ok(SyncEvent::CallReady{call,..})if call.call_id=="same-call");let leaves=s.http.log.lock().unwrap().iter().filter(|(path,token)|path=="/calls/same-call/leave"&&token=="original-authority").count();ready&&c.is_active()&&(if same_user{leaves==0}else{leaves==1})
}
async fn ready_before_ui_replacement()->bool{
 let(s,mut rx)=setup();let first=gate(&s,"/chats/old/calls",answer("old-call","old"));let a=attempt_for(&s,"old".into(),None);command(&s,start(&a,"old"));if !spin(||first.began.load(Ordering::SeqCst)).await{return false}first.release.notify_one();if !spin(||s.in_call.lock().unwrap().as_deref()==Some("old-call")).await{return false}
 // Ready is queued, but the UI still only knows its opaque pending attempt.
 a.cancel();let second=gate(&s,"/chats/new/calls",json!({"fixture_reject":true}));let b=attempt_for(&s,"new".into(),None);command(&s,start(&b,"new"));if !spin(||second.began.load(Ordering::SeqCst)).await{return false}second.release.notify_one();for _ in 0..100{tokio::task::yield_now().await;}
 let errors=std::iter::from_fn(||rx.try_recv().ok()).filter(|event|matches!(event,SyncEvent::CallError{..})).count();errors==1&&s.http.log.lock().unwrap().iter().filter(|(path,_)|path=="/calls/old-call/leave").count()==1
}
async fn pending_join_cancel_once()->bool{
 let(s,_)=setup();let response=gate(&s,"/calls/known/join",json!({"call":{"call_id":"known","chat_id":"same"}}));let a=attempt_for(&s,"same".into(),Some("known".into()));command(&s,join(&a,"known"));if !spin(||response.began.load(Ordering::SeqCst)).await{return false}
 command(&s,ClientCommand::LeaveCall{attempt:a.clone(),call_id:"known".into()});response.release.notify_one();for _ in 0..100{tokio::task::yield_now().await;}
 s.http.log.lock().unwrap().iter().filter(|(path,_)|path=="/calls/known/leave").count()==1&&!s.requests.lock().unwrap().iter().any(|path|path=="/calls/known/leave")
}
#[tokio::main(flavor="current_thread")]async fn main(){let mut failed=0;macro_rules! check{($name:expr,$f:expr)=>{let ok=$f.await;println!("{} {}",if ok{"PASS"}else{"FAIL"},$name);if !ok{failed+=1}}}check!("late response cleans server with original authority",late_response());check!("same-call replacement membership is preserved",same_call_variant(false,false));check!("cleanup serializes before replacement server join",serial_cleanup());check!("ordinary start/presence control",ordinary());check!("queued old Start Join Leave cannot use new account",queued_commands());check!("current-account leave remains admitted",valid_leave());check!("same user re-adoption keeps shared membership",same_call_variant(true,false));check!("different user membership still receives A cleanup",same_call_variant(true,true));check!("preserved membership is cleaned after replacement request fails",preserved_then_failed());check!("malformed join response terminates pending call",invalid_join(false));check!("empty joined call ID is refused",invalid_join(true));check!("replacement chain retains a successful same-member owner",replacement_chain(true));check!("replacement chain cleans after last failure",replacement_chain(false));check!("retired inherited cleanup preserves same-user replacement",inherited_retirement(true));check!("retired inherited cleanup uses A for foreign user",inherited_retirement(false));check!("ready-before-UI resource transfers to failed replacement",ready_before_ui_replacement());check!("pending join cancel makes one cleanup attempt",pending_join_cancel_once());println!("RESULT tests=17 failed={failed}");std::process::exit(if failed==0{0}else{1})}
'''
REGISTRY_SAME_CALL=r'''
async fn same_call_variant(reopen:bool,different:bool)->bool{
 let(s,mut rx)=setup();let g=gate(&s,"/chats/same/calls",answer("same-call","same"));let a=CallAttempt::new("same".into(),None);command(&s,start(&a,"same"));if !spin(||g.began.load(Ordering::SeqCst)).await{return false}a.cancel();
 let(mut fresh,mut fresh_rx)=setup();if reopen{{let mut life=s.account_lifetime.lock().unwrap();life.close();s.retire_call();}*s.creds.write().unwrap()=None;Arc::get_mut(&mut fresh).unwrap().call_registry=s.call_registry.clone();}
 let target=if reopen{&fresh}else{&s};if different{target.creds.write().unwrap().as_mut().unwrap().user=Some(User{id:"B".into()});}
 let j=gate(target,"/calls/same-call/join",json!({"call":{"call_id":"same-call","chat_id":"same"}}));let b=attempt_for(target,"same".into(),Some("same-call".into()));command(target,join(&b,"same-call"));g.release.notify_one();if !spin(||j.began.load(Ordering::SeqCst)).await{return false}j.release.notify_one();for _ in 0..100{tokio::task::yield_now().await;}
 let target_rx=if reopen{&mut fresh_rx}else{&mut rx};let mut ready=0;while let Ok(e)=target_rx.try_recv(){if matches!(e,SyncEvent::CallReady{..}){ready+=1}}
 ready==1&&(if different{s.http.log.lock().unwrap().iter().any(|(p,t)|p=="/calls/same-call/leave"&&t=="original-authority")}else{s.http.log.lock().unwrap().is_empty()})&&target.in_call.lock().unwrap().as_deref()==Some("same-call")
}
'''

def main():
 parser=argparse.ArgumentParser();parser.add_argument('--tree',type=Path,default=REPO);parser.add_argument('--label',default='relay');parser.add_argument('--record-dir',type=Path);args=parser.parse_args();source=(args.tree/SOURCE).read_text();after='ClientCommand::StartCall {\n            attempt,' in source
 a=source.index('        ClientCommand::StartCall {');b=source.index('        ClientCommand::FetchCall {',a);arms=source[a:b]
 helper=''
 if after:
  a=source.index('async fn compensate_call_join(');helper=source[a:source.index('\n}',a)+2]
 methods=''
 if after:
  names=['begin_call','leave_call_authority','owns_call','publish_call_ready','publish_call_error','retire_call']
  names+=['call_user_id','preserve_call_cleanup','take_inherited_cleanup','cleanup_authority','settle_inherited_cleanup'] if 'fn preserve_call_cleanup(' in source else ['preserves_call']
  for name in names:
   prefix='    async fn ' if '    async fn '+name+'(' in source else '    fn '
   at=source.index(prefix+name+'(');methods+=source[at:source.index('\n    }',at)+6]+'\n'
 fixture=PRELUDE
 registry='struct CallRegistry {' in source
 if registry:
  a=source.index('#[derive(Clone, PartialEq, Eq)]\nstruct CallCleanupTarget') if 'struct CallCleanupTarget {' in source else source.index('struct RegisteredCall {');declarations=source[a:source.index('\nstruct Shared {',a)]
  if 'struct CallCleanupTarget {' in source:
   declarations+='\n'+source[source.index('struct RetiredCall {'):source.index('\n}',source.index('struct RetiredCall {'))+2]
   declarations+='\n#[path='+json.dumps(str((args.tree/'crates/arbos-mac/src/sync/call_requests.rs').resolve()))+']mod call_requests;use call_requests::{CallRequests,CallRequestTurn};\n'
   declarations+='\ntrait AdmittedEvents{fn send_admitted(&self,life:&WorkLifetime,event:SyncEvent)->Result<(),()>;}impl AdmittedEvents for mpsc::UnboundedSender<SyncEvent>{fn send_admitted(&self,life:&WorkLifetime,event:SyncEvent)->Result<(),()>{if life.ticket().is_none(){return Err(())}self.send(event).map_err(|_|())}}\n'
  fixture=fixture.replace('struct Shared{',declarations+'\nstruct Shared{session_owner:WorkTicket,call_registry:Arc<CallRegistry>,',1)
  fixture=fixture.replace('let(tx,rx)=mpsc::unbounded_channel();let gates=', 'let(tx,rx)=mpsc::unbounded_channel();let life=WorkLifetime::default();let session_owner=life.ticket().unwrap();let gates=',1)
  fixture=fixture.replace('Shared{account_lifetime:Mutex::new(WorkLifetime::default()),','Shared{session_owner,call_registry:Arc::new(CallRegistry::new("http://synthetic.invalid".into())),account_lifetime:Mutex::new(life),',1)
  admitted=' async fn call_json_status_admitted(&self,authority:CallAuthority,_:reqwest::Method,p:&str,v:Option<Value>,e:&[(&str,String)])->Result<((),Value,CallAuthority)>{let (_,v)=self.post_json_status(p,v.unwrap_or(Value::Null),e).await?;Ok(((),v,authority))}\n'
  fixture=fixture.replace(' async fn post_json(&self,',admitted+' async fn post_json(&self,',1)
 at=fixture.index('fn command(shared:');fixture=fixture[:at-2]+methods+fixture[at-2:] if False else fixture
 # Insert methods inside the existing Shared impl, before its closing brace.
 at=fixture.index('\n}\nfn setup');fixture=fixture[:at]+methods+fixture[at:]
 tests=TESTS
 tests=tests.replace('CAPTURE_RETIRE','s.retire_call()' if 'struct CallCleanupTarget {' in source else '{s.retire_call();Vec::new()}')
 if registry:
  a=tests.index('async fn same_call_variant(');b=tests.index('async fn serial_cleanup()',a)
  tests=tests[:a]+REGISTRY_SAME_CALL+tests[b:]
  tests=tests.replace('CallAttempt::new("same".into(),Some("same-call".into()))','attempt_for(target,"same".into(),Some("same-call".into()))')
 fixture+=arms+tests+'\n'+helper
 fixture=fixture.replace('CallAttempt::new(', 'attempt_for(&s,')
 for a,b in {'ATTEMPT_PATH':json.dumps(str((args.tree/'crates/arbos-mac/src/call_attempt.rs').resolve())),'CMD_ATTEMPT':'attempt:CallAttempt,' if after else '', 'PASS_ATTEMPT':'attempt:a.clone(),' if after else ''}.items():fixture=fixture.replace(a,b)
 support=f'\ntype Id=String;\nmod sync {{ #[path = {json.dumps(str((args.tree/"crates/arbos-mac/src/sync/work_lifetime.rs").resolve()))}] pub mod work_lifetime; }}\nfn new_attempt(chat:String,id:Option<String>)->CallAttempt{{CallAttempt::new(chat,id,sync::work_lifetime::WorkLifetime::default().ticket().unwrap(),sync::work_lifetime::WorkLifetime::default().ticket().unwrap())}}\n'
 fixture=fixture+support+'use sync::work_lifetime::{WorkLifetime,WorkTicket};\n#[derive(Clone)]struct CallAuthority{owner:WorkTicket,CAP_USER_FIELD bearer:String}\nfn attempt_for(s:&Shared,chat:String,id:Option<String>)->CallAttempt{CallAttempt::new(chat,id,WorkLifetime::default().ticket().unwrap(),s.account_lifetime.lock().unwrap().ticket().unwrap())}\n'
 fixture=fixture.replace('CAP_USER_FIELD','user_id:Option<String>,' if 'user_id: Option<String>' in source else '')
 start=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='arbos-call-relay-')as temp:
  temp=Path(temp);(temp/'main.rs').write_text(fixture);(temp/'Cargo.toml').write_text('[workspace]\n[package]\nname="call-relay-check"\nversion="0.0.0"\nedition="2021"\n[[bin]]\nname="relay-check"\npath="main.rs"\n[dependencies]\ntokio={version="=LOCKED_TOKIO",features=["sync","rt","macros","time"]}\nserde={version="1",features=["derive"]}\nserde_json="1"\nanyhow="1"\nfutures="0.3"\n'.replace('LOCKED_TOKIO',dependency_version(REPO,'tokio')))
  env={'PATH':os.environ['PATH'],'HOME':str(temp),'TMPDIR':str(temp),'CARGO_HOME':str(Path.home()/'.cargo'),'RUSTUP_HOME':str(Path.home()/'.rustup'),'CARGO_NET_OFFLINE':'true','CARGO_TARGET_DIR':str(temp/'target')}
  compile=subprocess.run(fence(['cargo','build','--offline','--manifest-path',str(temp/'Cargo.toml')]),env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
  if compile.returncode:output=compile.stdout;code=compile.returncode
  else:
   p=subprocess.run(fence([str(temp/'target/debug/relay-check')]),env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True,timeout=10);output=p.stdout;code=p.returncode
 result={'exit_code':code,'compile_exit':compile.returncode,'wall_seconds':time.monotonic()-start,'source_sha256':hashlib.sha256(source.encode()).hexdigest(),'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'output':output}
 record(args.record_dir,args.label,result,output);print(json.dumps(result));return code
if __name__=='__main__':raise SystemExit(main())

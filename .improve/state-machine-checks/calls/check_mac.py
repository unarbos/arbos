"""Compile exact AppState call completion methods with standard-library fakes."""
from pathlib import Path
import argparse,hashlib,json,os,re,subprocess,tempfile,time
ROOT=Path(__file__).resolve().parent
from support import repository, dependency_version, fence, record
REPO=repository(ROOT)
PATH=Path('crates/arbos-mac/src/app.rs')
def method(text,name):
 matches=list(re.finditer(r'^    (?:pub )?fn '+name+r'\(',text,re.M));assert len(matches)==1,(name,len(matches))
 start=matches[0].start();return text[start:text.index('\n    }',matches[0].end())+6]
PRELUDE=r'''
use std::collections::HashMap;
use std::cell::RefCell;
use std::rc::Rc;
use std::marker::PhantomData;
#[derive(Clone,Debug,PartialEq,Eq)]enum CallKind{Voice,Video}
impl Default for CallKind{fn default()->Self{Self::Voice}}
impl CallKind{fn label(&self)->&str{"voice call"}}
#[derive(Clone,Debug,Default)]struct Call{call_id:String,chat_id:String,kind:CallKind,token:Option<String>,livekit_url:String,can_speak:Option<bool>}
impl Call{fn space_id(&self)->&str{&self.chat_id}}
#[derive(Clone,Debug)]struct Reachable;
fn reachable_line(_: &[Reachable])->Option<String>{None}
fn now_ms()->u64{0}
#[derive(Clone,Debug,PartialEq)]enum CallStatus{Connecting,Connected,Failed(String)}
#[derive(Clone,Debug)]struct ActiveCall{call:Call,status:CallStatus,muted:bool,camera:bool,participants:Vec<()>,local_video:Option<()>,joined_at:u64,expanded:bool,notice:Option<String>,outgoing:bool,reachable:Option<String>,agent_seeing:bool,ATTEMPT_FIELD}
struct Context<T>(PhantomData<T>);impl<T>Context<T>{fn notify(&mut self){}}
#[derive(Default)]struct Sender<T>(Rc<RefCell<Vec<T>>>);impl<T>Sender<T>{fn send(&self,v:T)->Result<(),()>{self.0.borrow_mut().push(v);Ok(())}}
impl<T> Clone for Sender<T>{fn clone(&self)->Self{Self(self.0.clone())}}
enum CallCmd{Join{url:String,token:String,camera:bool,JOIN_FIELD},SetMuted MUTED_FIELD,Leave LEAVE_FIELD}
enum ClientCommand{SystemLine{chat_id:String,text:String,call_id:Option<String>},LeaveCall{call_id:String,LEAVE_COMMAND_FIELD}}
struct CallEnded;struct Me;impl Me{fn display_name(&self)->String{"Person".into()}}
fn drop_active_call_images(_:ActiveCall,_:&mut Context<AppState>){}
struct AppState{my_call:Option<ActiveCall>,calls_in_chat:HashMap<String,Call>,incoming_call:Option<()>,call_commands:Sender<CallCmd>,commands:Vec<ClientCommand>,notices:Vec<String>,call_account:WorkLifetime,call_backend:Option<WorkTicket>,writes:usize}
impl AppState{fn command(&mut self,c:ClientCommand){self.commands.push(c)}fn push_notice(&mut self,s:&str){self.notices.push(s.into())}fn sync_tones(&mut self){}fn me_sender(&self)->Me{Me}fn on_call_ended(&mut self,_:CallEnded,_:&mut Context<Self>){self.writes+=1;self.my_call=None}
'''
TESTS=r'''
}
fn call(id:&str)->Call{Call{call_id:id.into(),chat_id:"chat".into(),token:Some("synthetic".into()),..Default::default()}}
fn state(id:Option<&str>,attempt:&CallAttempt)->AppState{AppState{my_call:id.map(|id|ActiveCall{call:call(id),status:CallStatus::Connecting,muted:false,camera:false,participants:vec![],local_video:None,joined_at:0,expanded:true,notice:None,outgoing:false,reachable:None,agent_seeing:false,ATTEMPT_VALUE}),calls_in_chat:HashMap::new(),incoming_call:None,call_commands:Sender(Rc::default()),commands:vec![],notices:vec![],call_account:WorkLifetime::default(),call_backend:None,writes:0}}
fn main(){let mut failures=0;let mut cx=Context(PhantomData);
let old=CallAttempt::new("chat".into(),None);old.cancel();let mut idle=state(None,&old);idle.on_call_ready(READY_OLD call("old"),true,vec![],&mut cx);let ok=idle.my_call.is_none()&&idle.call_commands.0.borrow().is_empty();println!("{} canceled REST cannot resurrect",if ok{"PASS"}else{"FAIL"});if !ok{failures+=1}
let current=CallAttempt::new("chat".into(),None);let mut replacement=state(Some("new"),&current);replacement.on_call_ready(READY_OLD call("old"),true,vec![],&mut cx);let ok=replacement.my_call.as_ref().map(|c|c.call.call_id.as_str())==Some("new")&&replacement.call_commands.0.borrow().is_empty()&&replacement.commands.is_empty();println!("{} old REST cannot replace current call",if ok{"PASS"}else{"FAIL"});if !ok{failures+=1}
let active=CallAttempt::new("chat".into(),None);let mut normal=state(Some("pending"),&active);normal.on_call_ready(READY_ACTIVE call("ok"),true,vec![],&mut cx);let ok=normal.my_call.as_ref().map(|c|c.call.call_id.as_str())==Some("ok")&&normal.call_commands.0.borrow().len()==1&&normal.commands.len()==1;println!("{} ordinary completion control",if ok{"PASS"}else{"FAIL"});if !ok{failures+=1}
let mut account=WorkLifetime::default();let owner=account.ticket().unwrap();let receipt=new_owned_attempt("chat".into(),None,owner);let mut m=state(Some("replacement"),&current);m.call_account=account;m.call_account.close();m.call_account.reopen();m.on_call_left(receipt,CallEnded,&mut cx);let ok=m.writes==0&&m.my_call.is_some();println!("{} queued receipt rejected after same-device readoption",if ok{"PASS"}else{"FAIL"});if !ok{failures+=1}
m.writes=0;let owner=m.call_account.ticket().unwrap();let receipt=new_owned_attempt("chat".into(),None,owner);receipt.cancel();m.my_call=None;m.on_call_left(receipt,CallEnded,&mut cx);let ok=m.writes==1;println!("{} same-account token rotation preserves receipt control",if ok{"PASS"}else{"FAIL"});if !ok{failures+=1}
let backend=WorkLifetime::default().ticket().unwrap();let mut m=state(Some("ongoing"),&current);m.call_backend=Some(backend);let ui=m.call_account.ticket().unwrap();m.on_call_account(backend,&mut cx);let ok=m.my_call.is_some()&&m.call_account.accepts(ui);println!("{} repeated backend ticket preserves ongoing call",if ok{"PASS"}else{"FAIL"});if !ok{failures+=1}
println!("RESULT tests=6 failed={failures}");std::process::exit(if failures==0{0}else{1});}
'''
BASE_ATTEMPT=r'''
#[derive(Clone,Debug)]struct CallAttempt;impl CallAttempt{fn new(_:String,_:Option<String>)->Self{Self}fn cancel(&self){}}
'''

def main():
 parser=argparse.ArgumentParser();parser.add_argument('--tree',type=Path,default=REPO);parser.add_argument('--label',default='mac');parser.add_argument('--record-dir',type=Path);args=parser.parse_args()
 source=(args.tree/PATH).read_text();after='pub attempt: CallAttempt' in source
 receipt=method(source,'on_call_left') if after else 'fn on_call_left(&mut self,_attempt:CallAttempt,ended:CallEnded,cx:&mut Context<Self>){'+re.search(r'SyncEvent::CallEnded\(ended\) => (self\.on_call_ended\(ended, cx\)),',source).group(1)+';}'
 adoption=method(source,'on_call_account') if after else 'fn on_call_account(&mut self,_owner:WorkTicket,_cx:&mut Context<Self>){}'
 fixture=PRELUDE+method(source,'on_call_ready')+(method(source,'leave_call') if after else '')+receipt+adoption+TESTS
 replacements={'MUTED_FIELD':'{attempt:CallAttempt,muted:bool}' if after else '(bool)', 'ATTEMPT_FIELD':'attempt:CallAttempt,' if after else '', 'ATTEMPT_VALUE':'attempt:attempt.clone(),' if after else '', 'JOIN_FIELD':'attempt:CallAttempt,' if after else '', 'LEAVE_FIELD':'{attempt:CallAttempt}' if after else '', 'LEAVE_COMMAND_FIELD':'attempt:CallAttempt,' if after else '', 'READY_OLD':'old.clone(),' if after else '', 'READY_ACTIVE':'active.clone(),' if after else ''}
 for a,b in replacements.items():fixture=fixture.replace(a,b)
 token=args.tree/'crates/arbos-mac/src/call_attempt.rs'
 fixture+='\n'+(f'#[path = {json.dumps(str(token.resolve()))}] mod call_attempt; use call_attempt::CallAttempt;' if after else BASE_ATTEMPT)
 support=f'\nmod sync {{ #[path = {json.dumps(str((args.tree/"crates/arbos-mac/src/sync/work_lifetime.rs").resolve()))}] pub mod work_lifetime; }}\nuse sync::work_lifetime::{{WorkLifetime,WorkTicket}};\n'
 fixture=fixture.replace('CallAttempt::new(', 'new_attempt(')+support
 fixture+='fn new_attempt(chat:String,id:Option<String>)->CallAttempt{new_owned_attempt(chat,id,WorkLifetime::default().ticket().unwrap())}\n'
 fixture+='fn new_owned_attempt(chat:String,id:Option<String>,owner:WorkTicket)->CallAttempt{CallAttempt::new(chat,id'+(',owner,WorkLifetime::default().ticket().unwrap()' if after else '')+')}\n'
 start=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='arbos-mac-calls-')as temp:
  temp=Path(temp);(temp/'main.rs').write_text(fixture);env={'PATH':os.environ['PATH'],'HOME':str(temp),'TMPDIR':str(temp)}
  (temp/'Cargo.toml').write_text('[workspace]\n[package]\nname="call-completion-check"\nversion="0.0.0"\nedition="2021"\n[[bin]]\nname="check"\npath="main.rs"\n[dependencies]\ntokio={version="=LOCKED_TOKIO",features=["sync"]}\n'.replace('LOCKED_TOKIO',dependency_version(REPO,'tokio')))
  env.update({'CARGO_HOME':str(Path.home()/'.cargo'),'RUSTUP_HOME':str(Path.home()/'.rustup'),'CARGO_NET_OFFLINE':'true','CARGO_TARGET_DIR':str(temp/'target')})
  compiler=subprocess.run(fence(['cargo','build','--offline','--manifest-path',str(temp/'Cargo.toml')]),env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
  if compiler.returncode:output=compiler.stdout;code=compiler.returncode
  else:
   p=subprocess.run(fence([str(temp/'target/debug/check')]),env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True,timeout=10);output=p.stdout;code=p.returncode
 result={'exit_code':code,'compile_exit':compiler.returncode,'wall_seconds':time.monotonic()-start,'source_sha256':hashlib.sha256(source.encode()).hexdigest(),'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'output':output}
 record(args.record_dir,args.label,result,output);print(json.dumps(result));return code
if __name__=='__main__':raise SystemExit(main())

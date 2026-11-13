"""Actual media engine loop with bounded synthetic connect/audio collaborators."""
from pathlib import Path
import argparse,hashlib,json,os,re,subprocess,tempfile,time
ROOT=Path(__file__).resolve().parent
from support import repository, dependency_version, fence, record
REPO=repository(ROOT)
SOURCE=Path('crates/arbos-mac/src/calls.rs')
PRELUDE=r'''
use std::collections::HashMap;use std::sync::{Arc,Mutex,OnceLock};use std::time::Duration;use tokio::sync::{mpsc,Notify};
#[derive(Debug,Clone)]struct RenderImage;
#[path=ATTEMPT_PATH]mod call_attempt;use call_attempt::CallAttempt;
#[derive(Clone)]struct Gate{release:Arc<Notify>}
static LOG:OnceLock<Mutex<Vec<String>>>=OnceLock::new();static GATES:OnceLock<Mutex<HashMap<String,Gate>>>=OnceLock::new();
fn log(s:String){LOG.get_or_init(Default::default).lock().unwrap().push(s)}
fn entries()->Vec<String>{LOG.get_or_init(Default::default).lock().unwrap().clone()}
struct Mic;impl Mic{fn mute(&self){}fn unmute(&self){}}
struct Room{token:String,_tx:mpsc::UnboundedSender<RoomEvent>}
impl Room{async fn close(&self)->Result<(),()>{log(format!("closed:{}",self.token));Ok(())}}
struct LocalVideoTrack;type LocalAudioTrack=Mic;struct PlatformAudio;
fn session(token:&str)->(Session,mpsc::UnboundedReceiver<RoomEvent>){let(tx,rx)=mpsc::unbounded_channel();(Session{room:Room{token:token.into(),_tx:tx},mic:None,camera:None,_audio:None,video_tasks:HashMap::new(),CLOSED_FIELD},rx)}
struct RoomEvent;
struct ConnectGuard(String,bool);impl Drop for ConnectGuard{fn drop(&mut self){if self.1{log(format!("dropped:{}",self.0));}}}
async fn connect(_: &str,token:&str,_:bool,_:&EVENTS_TYPE CONNECT_ATTEMPT)->Result<(Session,mpsc::UnboundedReceiver<RoomEvent>),String>{
 log(format!("begin:{token}"));let mut guard=ConnectGuard(token.into(),true);let gate=GATES.get_or_init(Default::default).lock().unwrap().get(token).cloned();if let Some(gate)=gate{gate.release.notified().await;}
 let(s,rx)=session(token);guard.1=false;Ok((s,rx))
}
async fn end_session(mut s:Session){let _=s.room.close().await;CLOSED_SET}
async fn publish_camera(_: &Room,_:&EVENTS_TYPE)->Result<CameraPublish,String>{Ok(CameraPublish{track:LocalVideoTrack,stop:Arc::new(std::sync::atomic::AtomicBool::new(false))})}
async fn stop_camera(_: &Room,_:CameraPublish){}
fn on_room_event(_:&mut Session,_:RoomEvent,_:&EVENTS_TYPE){}
'''
TESTS=r'''
async fn spin(f:impl Fn()->bool)->bool{for _ in 0..1000{if f(){return true}tokio::task::yield_now().await;}f()}
async fn connected(rx:&mut mpsc::UnboundedReceiver<EVENT_RX>)->bool{tokio::time::timeout(Duration::from_secs(2),async{while let Some(e)=rx.recv().await{if matches!(EVENT_VALUE,CallEvent::Connected){return true}}false}).await.unwrap_or(false)}
fn join(a:&CallAttempt,t:&str)->CallCmd{CallCmd::Join{JOIN_ATTEMPT url:"synthetic.invalid".into(),token:t.into(),camera:false}}
fn leave(a:&CallAttempt)->CallCmd{LEAVE_COMMAND}
async fn case_cancel_before()->bool{LOG.get_or_init(Default::default).lock().unwrap().clear();let(tx,cmd)=mpsc::unbounded_channel();let(ev,mut rx)=mpsc::unbounded_channel();let task=tokio::spawn(run(cmd,ev));let a=CallAttempt::new("old".into(),None);a.cancel();tx.send(join(&a,"before")).unwrap();for _ in 0..100{tokio::task::yield_now().await;}let ok=!entries().iter().any(|s|s=="begin:before")&&rx.try_recv().is_err();task.abort();let _=task.await;ok}
async fn case_cancel_during()->bool{LOG.get_or_init(Default::default).lock().unwrap().clear();let gate=Gate{release:Arc::new(Notify::new())};GATES.get_or_init(Default::default).lock().unwrap().insert("during".into(),gate.clone());let(tx,cmd)=mpsc::unbounded_channel();let(ev,mut rx)=mpsc::unbounded_channel();let task=tokio::spawn(run(cmd,ev));let a=CallAttempt::new("old".into(),None);tx.send(join(&a,"during")).unwrap();if !spin(||entries().contains(&"begin:during".into())).await{return false};a.cancel();for _ in 0..100{tokio::task::yield_now().await;}let promptly_dropped=entries().contains(&"dropped:during".into());gate.release.notify_one();for _ in 0..100{tokio::task::yield_now().await;}let mut stale=false;while let Ok(e)=rx.try_recv(){stale|=matches!(EVENT_VALUE,CallEvent::Connected)}tx.send(leave(&a)).unwrap();for _ in 0..10{tokio::task::yield_now().await;}task.abort();let _=task.await;GATES.get().unwrap().lock().unwrap().remove("during");promptly_dropped&&!stale}
async fn case_old_leave()->bool{LOG.get_or_init(Default::default).lock().unwrap().clear();let(tx,cmd)=mpsc::unbounded_channel();let(ev,mut rx)=mpsc::unbounded_channel();let task=tokio::spawn(run(cmd,ev));let a=CallAttempt::new("old".into(),None);tx.send(join(&a,"old")).unwrap();if !connected(&mut rx).await{return false};let b=CallAttempt::new("new".into(),None);tx.send(join(&b,"new")).unwrap();if !connected(&mut rx).await{return false};tx.send(leave(&a)).unwrap();for _ in 0..100{tokio::task::yield_now().await;}let ok=!entries().contains(&"closed:new".into());tx.send(leave(&b)).unwrap();task.abort();let _=task.await;ok}
async fn case_control()->bool{LOG.get_or_init(Default::default).lock().unwrap().clear();let(tx,cmd)=mpsc::unbounded_channel();let(ev,mut rx)=mpsc::unbounded_channel();let task=tokio::spawn(run(cmd,ev));let a=CallAttempt::new("ok".into(),None);tx.send(join(&a,"ok")).unwrap();let connected=connected(&mut rx).await;tx.send(leave(&a)).unwrap();let closed=spin(||entries().contains(&"closed:ok".into())).await;task.abort();let _=task.await;connected&&closed}
#[tokio::main(flavor="current_thread")]async fn main(){let mut failures=0;for(name,f)in[("cancel before media join",case_cancel_before as fn()->_), ("cancel drops in-flight media connect",case_cancel_during as fn()->_), ("old leave cannot close replacement",case_old_leave as fn()->_), ("ordinary join leave control",case_control as fn()->_)]{let ok=f().await;println!("{} {name}",if ok{"PASS"}else{"FAIL"});if !ok{failures+=1}}println!("RESULT tests=6 failed={failures}");std::process::exit(if failures==0{0}else{1})}
'''
# Async fn opaque future types differ, so invoke a bounded local macro instead.
TESTS=TESTS[:TESTS.index('#[tokio::main')]+r'''
async fn case_camera_guard()->bool{let stop=Arc::new(std::sync::atomic::AtomicBool::new(false));drop(CameraPublish{track:LocalVideoTrack,stop:stop.clone()});stop.load(std::sync::atomic::Ordering::Relaxed)}
async fn case_session_guard()->bool{let(s,_)=session("drop-session");drop(s);spin(||entries().contains(&"closed:drop-session".into())).await}
#[tokio::main(flavor="current_thread")]async fn main(){let mut failures=0;
macro_rules! check{($name:expr,$run:expr)=>{let ok=$run.await;println!("{} {}",if ok{"PASS"}else{"FAIL"},$name);if !ok{failures+=1}}}
check!("cancel before media join",case_cancel_before());check!("cancel drops in-flight media connect",case_cancel_during());check!("old leave cannot close replacement",case_old_leave());check!("ordinary join leave control",case_control());check!("camera ownership stops capture on drop",case_camera_guard());check!("session ownership closes on drop",case_session_guard());println!("RESULT tests=6 failed={failures}");std::process::exit(if failures==0{0}else{1})}
'''
def block(s,start):
 a=s.index(start);return s[a:s.index('\n}',a)+2]
def main():
 parser=argparse.ArgumentParser();parser.add_argument('--tree',type=Path,default=REPO);parser.add_argument('--label',default='engine');parser.add_argument('--record-dir',type=Path);args=parser.parse_args();s=(args.tree/SOURCE).read_text();after='pub struct AttemptEvent' in s
 enums=block(s,'#[derive(Debug)]\npub enum CallCmd')+'\n'+block(s,'#[derive(Debug, Clone)]\npub enum CallEvent')
 resources=block(s,'struct Session {')+'\n'+block(s,'struct CameraPublish {')
 for name in ['Session','CameraPublish']:
  if 'impl Drop for '+name in s:resources+='\n'+block(s,'impl Drop for '+name+' {')
 fixture=PRELUDE+'\n'+resources+'\n'+enums+'\n'+(block(s,'#[derive(Debug, Clone)]\npub struct AttemptEvent')+'\n'+block(s,'#[derive(Clone)]\nstruct CallEvents')+'\n'+block(s,'impl CallEvents') if after else '')+'\n'+block(s,'pub async fn run(')+TESTS
 replacements={'CLOSED_FIELD':'closed:false,' if after else '', 'CLOSED_SET':'s.closed=true;' if after else '', 'ATTEMPT_PATH':json.dumps(str((args.tree/'crates/arbos-mac/src/call_attempt.rs').resolve())),'EVENTS_TYPE':'CallEvents' if after else 'mpsc::UnboundedSender<CallEvent>','CONNECT_ATTEMPT':',_:&CallAttempt' if after else '', 'EVENT_RX':'AttemptEvent' if after else 'CallEvent','EVENT_VALUE':'e.event' if after else 'e','JOIN_ATTEMPT':'attempt:a.clone(),' if after else '', 'LEAVE_COMMAND':'CallCmd::Leave{attempt:a.clone()}' if after else 'CallCmd::Leave'}
 for a,b in replacements.items():fixture=fixture.replace(a,b)
 if "    room: Arc<Room>," in s:
  fixture=fixture.replace('room:Room{token:token.into(),_tx:tx}','room:Arc::new(Room{token:token.into(),_tx:tx})')
 support=f'\nmod sync {{ #[path = {json.dumps(str((args.tree/"crates/arbos-mac/src/sync/work_lifetime.rs").resolve()))}] pub mod work_lifetime; }}\nfn new_attempt(chat:String,id:Option<String>)->CallAttempt{{CallAttempt::new(chat,id,sync::work_lifetime::WorkLifetime::default().ticket().unwrap(),sync::work_lifetime::WorkLifetime::default().ticket().unwrap())}}\n'
 fixture=fixture.replace('CallAttempt::new(', 'new_attempt(')+support
 start=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='arbos-call-engine-')as temp:
  temp=Path(temp);(temp/'main.rs').write_text(fixture);(temp/'Cargo.toml').write_text('[workspace]\n[package]\nname="call-engine-check"\nversion="0.0.0"\nedition="2021"\n[[bin]]\nname="engine-check"\npath="main.rs"\n[dependencies]\ntokio={version="=LOCKED_TOKIO",features=["sync","rt","macros","time"]}\n'.replace('LOCKED_TOKIO',dependency_version(REPO,'tokio')))
  env={'PATH':os.environ['PATH'],'HOME':str(temp),'TMPDIR':str(temp),'CARGO_HOME':str(Path.home()/'.cargo'),'RUSTUP_HOME':str(Path.home()/'.rustup'),'CARGO_NET_OFFLINE':'true','CARGO_TARGET_DIR':str(temp/'target')}
  compile=subprocess.run(fence(['cargo','build','--offline','--manifest-path',str(temp/'Cargo.toml')]),env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
  if compile.returncode:output=compile.stdout;code=compile.returncode
  else:
   p=subprocess.run(fence([str(temp/'target/debug/engine-check')]),env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True,timeout=10);output=p.stdout;code=p.returncode
 result={'exit_code':code,'compile_exit':compile.returncode,'wall_seconds':time.monotonic()-start,'source_sha256':hashlib.sha256(s.encode()).hexdigest(),'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'output':output}
 record(args.record_dir,args.label,result,output);print(json.dumps(result));return code
if __name__=='__main__':raise SystemExit(main())

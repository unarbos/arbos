"""Actual UI admission/command/timer bodies with local collaborators; no GPUI runtime."""
from pathlib import Path
import argparse,hashlib,json,os,re,subprocess,time,sys,tempfile,shutil
HERE=Path(__file__).resolve().parent;REPO=HERE.parents[2]
parser=argparse.ArgumentParser();parser.add_argument('--tree',type=Path,default=REPO);parser.add_argument('--record-dir',type=Path);args=parser.parse_args();TREE=args.tree.resolve();OUTPUT=args.record_dir.resolve() if args.record_dir else Path(tempfile.mkdtemp(prefix='arbos-session-ui-results-'));OUTPUT.mkdir(parents=True,exist_ok=True)
APP=TREE/'crates/arbos-mac/src/app.rs';source=APP.read_text();dependencies=[APP,*[TREE/f'crates/arbos-mac/src/sync/{name}.rs' for name in ['work_lifetime','session_scope','session_events']]];source_hashes={str(p.relative_to(TREE)):hashlib.sha256(p.read_bytes()).hexdigest() for p in dependencies}
def method(signature):
 a=source.index('    '+signature);return source[a:source.index('\n    }',a)+6]
start=source.index('            SyncEvent::Paired { device_id } => {');end=source.index('\n            SyncEvent::Features',start);paired=source[start:end]
a=source.index('            let go = this.update(cx, |state, _| {',source.index('    fn poll_connect_status('));a=source.index('{',a)+1;b=source.index('\n            });',a);delayed=source[a:b]
fixture='''use std::sync::Arc;use tokio::sync::mpsc;
'''
fixture+='mod sync {pub use crate::SyncEvent;\n'
for mod in ['work_lifetime','session_scope','session_events']:
 fixture+='#[path='+json.dumps(str((TREE/f'crates/arbos-mac/src/sync/{mod}.rs').resolve()))+']pub mod '+mod+';\n'
fixture+='}\nuse sync::work_lifetime::{WorkLifetime,WorkTicket};use sync::session_scope::UiSession;\n'
fixture+='''
#[derive(Debug)]struct FakeAttempt(WorkTicket);impl FakeAttempt{fn backend(&self)->WorkTicket{self.0}}
#[derive(Debug)]enum ClientCommand {SetReadReceipts{enabled:bool,persist:bool},StartCall{attempt:FakeAttempt},JoinCall{attempt:FakeAttempt},LeaveCall{attempt:FakeAttempt}, SyntheticWrite,Pair{code:String},SignIn{grant_code:String},SignOut,OutsideAgentStatus{project_id:String},Scoped{control:WorkTicket,account:Option<WorkTicket>,command:Box<ClientCommand>} }
#[derive(Debug)]pub enum SyncEvent {Paired{device_id:String},ForAccount{owner:WorkTicket,event:Box<SyncEvent>},ForCoordinator{owner:WorkTicket,event:Box<SyncEvent>},SessionStarted{control:WorkTicket,account:WorkTicket}}
mod draft_dm {pub fn names_draft(_: &super::ClientCommand)->bool{false}}
struct Context<T>(std::marker::PhantomData<T>);impl<T>Context<T>{fn notify(&mut self){}}
struct CallAccount;impl CallAccount{fn close(&self){}}
struct ConnectReveal{project_id:String}
struct AppState{local:LocalView,commands:mpsc::UnboundedSender<ClientCommand>,session:UiSession,account_scoped:bool,pairing:Option<()>,device_id:Option<String>,first_project_pending:bool,call_account:CallAccount,call_backend:Option<WorkTicket>,account:Option<()>,projects:Vec<()>,chats:Vec<()>,messages:Vec<String>,selected_project:Option<()>,people_directory:Vec<()>,people_results:Vec<()>,connect_reveal:Option<ConnectReveal>}
impl AppState {
 fn fixture(scoped:bool)->(Self,mpsc::UnboundedReceiver<ClientCommand>){let(tx,rx)=mpsc::unbounded_channel();(Self{local:FakeData::default(),commands:tx,session:UiSession::default(),account_scoped:scoped,pairing:Some(()),device_id:None,first_project_pending:false,call_account:CallAccount,call_backend:None,account:None,projects:vec![],chats:vec![],messages:vec![],selected_project:None,people_directory:vec![],people_results:vec![],connect_reveal:None},rx)}
 fn maybe_prompt_notifications(&self){}fn begin_onboarding(&self,_:&mut Context<Self>){}fn on_call_account(&mut self,owner:WorkTicket,_:&mut Context<Self>){self.call_backend=Some(owner);}fn push_notice(&self,_:String){}fn leave_call(&self,_:&mut Context<Self>){self.command(ClientCommand::SyntheticWrite);}
'''+method('pub(crate) fn command(')+'\n'+method('pub(crate) fn apply(')+'\n'+method('pub fn sign_out(')+'''
 fn apply_inner(&mut self,event:SyncEvent,cx:&mut Context<Self>){match event{
'''+paired+'''
 _=>{}}}
}
fn sync_event_label(_: &SyncEvent)->String{String::new()}
fn delayed_connect_step(state:&AppState,control:Option<WorkTicket>,account:Option<WorkTicket>,project_id:String)->bool{'''+delayed+'''}
#[cfg(test)]mod tests{use super::*;
 fn cx()->Context<AppState>{Context(std::marker::PhantomData)}
 fn account()->WorkTicket{WorkLifetime::default().ticket().unwrap()}
 fn bind(ui:&mut AppState,owner:WorkTicket){let control=ui.session.control.ticket().unwrap();ui.apply(SyncEvent::SessionStarted{control,account:owner},&mut cx());}
 fn paired(id:&str)->Box<SyncEvent>{Box::new(SyncEvent::Paired{device_id:id.into()})}
 #[test]fn fresh_session_does_not_show_a_data_while_b_is_empty(){let(mut ui,_)=AppState::fixture(true);bind(&mut ui,account());ui.projects.push(());ui.chats.push(());ui.messages.push("private-A-transcript".into());ui.session.begin_control();bind(&mut ui,account());assert!(ui.projects.is_empty()&&ui.chats.is_empty()&&ui.messages.is_empty());}
 #[test]fn call_cleanup_command_keeps_its_resource_owner_not_current_ui_owner(){let(mut ui,mut rx)=AppState::fixture(true);let a=account();let b=account();bind(&mut ui,b);ui.command(ClientCommand::LeaveCall{attempt:FakeAttempt(a)});assert!(matches!(rx.try_recv(),Ok(ClientCommand::Scoped{account:Some(owner),..})if owner==a));}
 #[test]fn local_device_account_data_does_not_survive_replacement(){let(mut ui,_)=AppState::fixture(true);bind(&mut ui,account());ui.local.0=true;ui.session.begin_control();bind(&mut ui,account());assert!(!ui.local.0);}
 #[test]fn queued_control_and_session_start_cannot_revive_signout(){let(mut ui,_)=AppState::fixture(true);let old=ui.session.control.ticket().unwrap();let a=account();ui.session.invalidate();ui.apply(SyncEvent::ForCoordinator{owner:old,event:paired("A")},&mut cx());ui.apply(SyncEvent::SessionStarted{control:old,account:a},&mut cx());assert!(ui.device_id.is_none()&&ui.pairing.is_some()&&ui.session.account().is_none());}
 #[test]fn queued_account_event_cannot_paint_replacement(){let(mut ui,_)=AppState::fixture(true);let a=account();bind(&mut ui,a);ui.session.invalidate();let b=account();bind(&mut ui,b);ui.apply(SyncEvent::ForAccount{owner:b,event:paired("B")},&mut cx());ui.apply(SyncEvent::ForAccount{owner:a,event:paired("A")},&mut cx());assert_eq!(ui.device_id.as_deref(),Some("B"));}
 #[test]fn current_session_command_keeps_original_owner_in_queue(){let(mut ui,mut rx)=AppState::fixture(true);let a=account();bind(&mut ui,a);ui.command(ClientCommand::SyntheticWrite);ui.session.invalidate();let b=account();bind(&mut ui,b);assert!(matches!(rx.try_recv(),Ok(ClientCommand::Scoped{account:Some(owner),..})if owner==a));assert_ne!(a,b);}
 #[test]fn current_session_data_and_coordinator_controls_work(){let(mut ui,mut rx)=AppState::fixture(true);let control=ui.session.control.ticket().unwrap();ui.apply(SyncEvent::ForCoordinator{owner:control,event:paired("preauth")},&mut cx());assert_eq!(ui.device_id.as_deref(),Some("preauth"));let a=account();bind(&mut ui,a);ui.command(ClientCommand::SyntheticWrite);assert!(matches!(rx.try_recv(),Ok(ClientCommand::Scoped{control:ticket,account:Some(owner),..})if ticket==control&&owner==a));}
 #[test]fn real_raw_events_and_unbound_data_commands_fail_closed(){let(mut ui,mut rx)=AppState::fixture(true);ui.apply(SyncEvent::Paired{device_id:"unowned".into()},&mut cx());ui.command(ClientCommand::SyntheticWrite);assert!(ui.device_id.is_none()&&rx.try_recv().is_err());}
 #[test]fn fixture_source_keeps_original_bare_contract(){let(mut ui,mut rx)=AppState::fixture(false);ui.apply(SyncEvent::Paired{device_id:"fixture".into()},&mut cx());ui.command(ClientCommand::SyntheticWrite);assert_eq!(ui.device_id.as_deref(),Some("fixture"));assert!(matches!(rx.try_recv(),Ok(ClientCommand::SyntheticWrite)));}
 #[test]fn local_signout_keeps_leave_before_invalidation_and_captures_old_account(){let(mut ui,mut rx)=AppState::fixture(true);let a=account();bind(&mut ui,a);let old_control=ui.session.control.ticket().unwrap();ui.sign_out(&mut cx());assert!(!ui.session.control.accepts(old_control));assert!(ui.session.account().is_none());assert!(matches!(rx.try_recv(),Ok(ClientCommand::Scoped{account:Some(owner),command,..})if owner==a&&matches!(*command,ClientCommand::SyntheticWrite)));assert!(matches!(rx.try_recv(),Ok(ClientCommand::Scoped{control,account:Some(owner),command})if owner==a&&ui.session.control.accepts(control)&&matches!(*command,ClientCommand::SignOut)));}
 #[test]fn delayed_network_intent_cannot_rebind_to_same_id_in_b(){let(mut ui,mut rx)=AppState::fixture(true);let a=account();bind(&mut ui,a);let captured=ui.session.control.ticket();ui.session.invalidate();let b=account();bind(&mut ui,b);ui.connect_reveal=Some(ConnectReveal{project_id:"same".into()});assert!(!delayed_connect_step(&ui,captured,Some(a),"same".into()));assert!(rx.try_recv().is_err());assert!(delayed_connect_step(&ui,ui.session.control.ticket(),Some(b),"same".into()));assert!(matches!(rx.try_recv(),Ok(ClientCommand::Scoped{account:Some(owner),command,..})if owner==b&&matches!(*command,ClientCommand::OutsideAgentStatus{..})));}
 #[test]fn sender_uses_borrowed_admission_without_relocking_and_rejects_retirement(){let(tx,mut rx)=mpsc::unbounded_channel();let life=Arc::new(std::sync::Mutex::new(WorkLifetime::default()));let owner=life.lock().unwrap().ticket().unwrap();let sender=sync::session_events::SessionEvents::new(tx,owner,life.clone());{let admitted=life.lock().unwrap();assert!(sender.send_admitted(&admitted,*paired("A")).is_ok());}assert!(matches!(rx.try_recv(),Ok(SyncEvent::ForAccount{owner:ticket,..})if ticket==owner));life.lock().unwrap().close();assert!(sender.send(*paired("late")).is_err());assert!(rx.try_recv().is_err());}
}
'''
rig_anchor='let _ = this.update(cx, |s, cx| s.add_agent(chat, pid, cx));'
if rig_anchor in source:
 rig='state.add_agent(chat, pid, cx);'
else:
 a=source.index('let _ = this.update(cx, |state, cx| {',source.index('if let Ok(spec) = std::env::var("ARBOS_RIG_ADD_AGENT")'))
 a=source.index('{',a)+1;b=source.index('\n                                });',a);rig=source[a:b]
fixture=fixture.replace(' fn maybe_prompt_notifications(&self){}',' fn add_agent(&self,_chat:String,pid:String,_cx:&mut Context<Self>){self.command(ClientCommand::OutsideAgentStatus{project_id:pid});} fn maybe_prompt_notifications(&self){}',1)
fixture+='\nfn delayed_rig_step(state:&AppState,control:Option<WorkTicket>,account:Option<WorkTicket>,chat:String,pid:String,cx:&mut Context<AppState>){'+rig+'}\n'
fixture+=r'''
#[cfg(test)]mod rig_origin_checks{use super::*;#[test]fn delayed_rig_mutation_keeps_original_ui_lifetime(){let(mut ui,mut rx)=AppState::fixture(true);let old=ui.session.control.ticket();ui.session.invalidate();let owner=WorkLifetime::default().ticket().unwrap();ui.apply(SyncEvent::SessionStarted{control:ui.session.control.ticket().unwrap(),account:owner},&mut Context(std::marker::PhantomData));delayed_rig_step(&ui,old,None,"same-chat".into(),"same-agent".into(),&mut Context(std::marker::PhantomData));assert!(rx.try_recv().is_err());delayed_rig_step(&ui,ui.session.control.ticket(),Some(owner),"same-chat".into(),"same-agent".into(),&mut Context(std::marker::PhantomData));assert!(matches!(rx.try_recv(),Ok(ClientCommand::Scoped{account:Some(current),..})if current==owner));}}
'''
fixture+=r'''
#[cfg(test)]mod same_control_origins {use super::*;
 fn bind(ui:&mut AppState,owner:WorkTicket){ui.apply(SyncEvent::SessionStarted{control:ui.session.control.ticket().unwrap(),account:owner},&mut Context(std::marker::PhantomData));}
 #[test]fn old_connect_callback_is_refused_when_backend_changes_under_same_control(){let(mut ui,mut rx)=AppState::fixture(true);let a=WorkLifetime::default().ticket().unwrap();bind(&mut ui,a);let old_control=ui.session.control.ticket();let b=WorkLifetime::default().ticket().unwrap();bind(&mut ui,b);ui.connect_reveal=Some(ConnectReveal{project_id:"same".into()});assert!(!delayed_connect_step(&ui,old_control,Some(a),"same".into()));assert!(rx.try_recv().is_err());assert!(delayed_connect_step(&ui,old_control,Some(b),"same".into()));assert!(rx.try_recv().is_ok());}
 #[test]fn old_rig_callback_is_refused_when_backend_changes_under_same_control(){let(mut ui,mut rx)=AppState::fixture(true);let a=WorkLifetime::default().ticket().unwrap();bind(&mut ui,a);let old_control=ui.session.control.ticket();let b=WorkLifetime::default().ticket().unwrap();bind(&mut ui,b);delayed_rig_step(&ui,old_control,Some(a),"same-chat".into(),"same-agent".into(),&mut Context(std::marker::PhantomData));assert!(rx.try_recv().is_err());delayed_rig_step(&ui,old_control,Some(b),"same-chat".into(),"same-agent".into(),&mut Context(std::marker::PhantomData));assert!(rx.try_recv().is_ok());}
}
'''
inventory=json.loads((HERE/'UI-RESET-INVENTORY.json').read_text())
a=source.index('pub struct AppState {');b=source.index('\n}',a)
fields=set(re.findall(r'^    (?:pub(?:\(crate\))? )?([a-z_]+):',source[a:b],re.M))
assert fields==set(inventory['account_fields_reset'])|set(inventory['retained_global_or_separately_owned']),'Classify new account UI fields before accepting reset coverage'
# Compile the whole actual reset method with synthetic field collaborators.
reset=method('fn reset_account_views(')
fixture=fixture.replace(' fn apply_inner(',reset+'\n fn apply_inner(',1)
a=fixture.index('struct AppState{');b=fixture.index('}\nimpl AppState',a);known=set(re.findall(r'\b([a-z_]+):',fixture[a:b]));extra_fields={};aliases=set()
for field in re.findall(r'self\.([a-z_]+)\.clear\(\)',reset):
 if field not in known:extra_fields[field]=('Vec<String>','vec![]')
for field,value in re.findall(r'self\.([a-z_]+) = ([^;]+);',reset):
 if field in known:continue
 if value=='None':kind='Option<String>'
 elif value in ['true','false']:kind='bool'
 elif value=='0' or value=='PIN_CAP_DEFAULT':kind='usize'
 elif value=='(0, 0)':kind='(i64,i64)'
 elif value=='Link::Connecting':kind='Link'
 elif value=='(LinkState::Live, 0)':kind='(LinkState,u64)'
 elif value.endswith('::default()'):kind=value.split('::')[0];aliases.add(kind)
 else:raise ValueError((field,value))
 extra_fields[field]=(kind,value)
extra_fields['notifier']=('FakeNotifier','FakeNotifier::default()')
extra_fields['read_receipts']=('bool','true');extra_fields['link_previews']=('bool','true');extra_fields['copy_request_seq']=('u64','99')
fixture=fixture.replace('struct AppState{','struct AppState{'+''.join(f'{name}:{kind},' for name,(kind,_) in extra_fields.items()),1)
fixture=fixture.replace('Self{local:FakeData::default(),commands:tx,','Self{'+''.join(f'{name}:{value},' for name,(_,value) in extra_fields.items())+'local:FakeData::default(),commands:tx,',1)
fixture+='\ntype LocalView=FakeData;\n#[derive(Default)]struct FakeData(bool);\n#[derive(Default)]struct FakeNotifier{pending:Vec<String>}impl FakeNotifier{fn reset_account(&mut self){self.pending.clear()}}\nenum Link{Connecting}enum LinkState{Live}const PIN_CAP_DEFAULT:usize=5;\n'+''.join(f'type {name}=FakeData;\n' for name in aliases)
# Real two-event revocation sequence, with UI effects replaced by local collaborators.
a=source.index('            SyncEvent::NeedsPairing { relay_url } => {');b=source.index('\n            SyncEvent::Paired',a);needs=source[a:b]
a=source.index('            SyncEvent::Account(account) => {');b=source.index('\n            SyncEvent::PeopleSearch',a);account_arm=source[a:b]
fixture=fixture.replace('match event{\n'+paired,'match event{\n'+needs+'\n'+account_arm+'\n'+paired,1)
fixture=fixture.replace('pub enum SyncEvent {Paired','pub enum SyncEvent {Account(Option<AccountView>),NeedsPairing{relay_url:String},Paired',1)
fixture=fixture.replace('account:Option<()>','account:Option<AccountView>').replace('pairing:Option<()>','pairing:Option<Pairing>').replace('pairing:Some(())','pairing:Some(Pairing::default())')
fixture=fixture.replace('enum ClientCommand { SyntheticWrite,','enum ClientCommand {SetReadReceipts{enabled:bool,persist:bool},SyntheticWrite,',1)
fixture=fixture.replace(' fn maybe_prompt_notifications(&self){}',' fn prefetch_directory(&self){}fn want_avatar(&self,_:String){}fn maybe_prompt_notifications(&self){}',1)
fixture+='\n#[derive(Debug)]pub struct AccountView{avatar_url:String}\n#[derive(Clone,Copy,Default)]enum SignInMode{#[default]GitHub}\n#[derive(Default)]struct Pairing{relay_url:String,mode:SignInMode,busy:bool,error:Option<String>,error_details_open:bool,flow:FakeData}\nmod sign_in{pub type Flow=super::FakeData;}\n'
fixture+=r'''
#[cfg(test)]mod account_reset_checks{use super::*;
 fn cx()->Context<AppState>{Context(std::marker::PhantomData)}
 fn bind(ui:&mut AppState,owner:WorkTicket){ui.apply(SyncEvent::SessionStarted{control:ui.session.control.ticket().unwrap(),account:owner},&mut cx());}
 #[test]fn same_session_replay_keeps_data_but_replacement_and_local_signout_clear_it(){let(mut ui,_)=AppState::fixture(true);let a=WorkLifetime::default().ticket().unwrap();bind(&mut ui,a);ui.messages.push("private-A".into());ui.secrets=FakeData(true);ui.notifier.pending.push("A-notification".into());bind(&mut ui,a);assert_eq!(ui.messages.len(),1);assert!(ui.secrets.0);assert_eq!(ui.notifier.pending.len(),1);ui.session.begin_control();bind(&mut ui,WorkLifetime::default().ticket().unwrap());assert!(ui.messages.is_empty()&&!ui.secrets.0&&ui.notifier.pending.is_empty());assert!(ui.read_receipts&&ui.link_previews);assert_eq!(ui.copy_request_seq,99);ui.messages.push("private-B".into());ui.sign_out(&mut cx());assert!(ui.messages.is_empty());}
 #[test]fn scoped_server_revocation_admits_both_tombstones_then_rejects_late_a(){let(mut ui,_)=AppState::fixture(true);let a=WorkLifetime::default().ticket().unwrap();bind(&mut ui,a);ui.account=Some(AccountView{avatar_url:"synthetic".into()});ui.messages.push("A-only".into());ui.apply(SyncEvent::ForAccount{owner:a,event:Box::new(SyncEvent::Account(None))},&mut cx());assert!(ui.account.is_none());assert_eq!(ui.session.account(),Some(a));ui.apply(SyncEvent::ForAccount{owner:a,event:Box::new(SyncEvent::NeedsPairing{relay_url:"http://synthetic.invalid".into()})},&mut cx());assert!(ui.session.account().is_none()&&ui.messages.is_empty()&&ui.pairing.is_some());ui.apply(SyncEvent::ForAccount{owner:a,event:Box::new(SyncEvent::Paired{device_id:"obsolete".into()})},&mut cx());assert!(ui.device_id.is_none());}
}
'''
scratch=tempfile.TemporaryDirectory(prefix='arbos-session-ui-build-');work=Path(scratch.name);(work/'src').mkdir(parents=True,exist_ok=True);(work/'src/lib.rs').write_text(fixture)
shutil.copyfile(HERE/'ui.Cargo.toml',work/'Cargo.toml');shutil.copyfile(HERE/'ui.Cargo.lock',work/'Cargo.lock')
env={'PATH':os.environ['PATH'],'HOME':str(work),'CARGO_HOME':str(Path.home()/'.cargo'),'RUSTUP_HOME':str(Path.home()/'.rustup'),'CARGO_BUILD_JOBS':'2','CARGO_INCREMENTAL':'0'}
t=time.monotonic();r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)','cargo','test','--offline','--locked','--manifest-path',str(work/'Cargo.toml'),'--','--test-threads=1'],env=env,cwd=REPO,text=True,capture_output=True,timeout=180)
(OUTPUT/'tests.log').write_text((r.stdout+r.stderr).replace(str(TREE),'<source-tree>').replace(str(work),'<fresh-build>'));assert source_hashes=={str(p.relative_to(TREE)):hashlib.sha256(p.read_bytes()).hexdigest() for p in dependencies},'Sources changed during UI gate';row={'sources':source_hashes,'exit_code':r.returncode,'seconds':time.monotonic()-t,'source_sha256':hashlib.sha256(source.encode()).hexdigest(),'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest()};m=re.search(r'test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;',r.stdout)
if m:row['counts']=dict(zip(['passed','failed','ignored'],map(int,m.groups())))
(OUTPUT/'GATES.json').write_text(json.dumps(row,indent=2)+'\n');print(json.dumps(row))

assert r.returncode==0 and row.get("counts",{}).get("passed")==21 and row["counts"]["failed"]==0,"Expected21 current UI checks"
scratch.cleanup()

// Synthetic effect collaborators only. Actual methods/helpers are extracted by check_backend.py.

use std::sync::{Arc,Mutex,RwLock,Weak};
use std::sync::atomic::{AtomicBool,AtomicUsize,AtomicU32,AtomicU64,Ordering};
use std::time::{Duration,Instant};
use anyhow::{anyhow,Context as _,Result};
use serde::{Deserialize,Serialize};
use serde_json::{json,Value};
use tokio::sync::mpsc;
#[derive(Clone,Debug,Default,Deserialize,Serialize)] pub struct AccountUser {id:String,login:String,name:String,avatar_url:String}
impl From<&AccountUser> for AccountUser {fn from(v:&AccountUser)->Self{v.clone()}}
#[derive(Clone,Debug)] struct DeviceCredentials {relay_url:String,device_id:String,access_token:String,refresh_token:Option<String>,expires_at:u64,user:Option<AccountUser>}
impl DeviceCredentials {fn needs_refresh(&self)->bool{true}}
#[derive(Deserialize)] #[serde(default)] struct TokenResponseDto {device_id:String,access_token:String,refresh_token:String,expires_in:i64,user:AccountUser}
impl Default for TokenResponseDto {fn default()->Self{Self{device_id:String::new(),access_token:String::new(),refresh_token:String::new(),expires_in:0,user:AccountUser::default()}}}
fn now_ms()->u64{1_700_000_000_000}
mod credentials {
 pub use super::AccountUser;
 use super::*;
 pub static WRITES:Mutex<Vec<String>>=Mutex::new(Vec::new());
 pub fn store(v:&DeviceCredentials)->Result<()>{WRITES.lock().unwrap().push(v.access_token.clone());Ok(())}
 pub fn clear(_: &str){WRITES.lock().unwrap().clear()}
}
#[derive(Clone)] struct Http {status:u16,body:Value}
struct Request {http:Http,bearer:String,path:String}
#[derive(Clone,Copy)] struct Status(u16);
impl Status {fn as_u16(self)->u16{self.0}fn is_success(self)->bool{(200..300).contains(&self.0)}}
struct Response(Http);
type Hook=Box<dyn FnOnce()+Send>;
static HTTP_HOOK:Mutex<Option<Hook>>=Mutex::new(None);
static MATRIX_HOOK:Mutex<Option<Hook>>=Mutex::new(None);
static SDK_CALLS:AtomicUsize=AtomicUsize::new(0);
static SDK_STOPS:AtomicUsize=AtomicUsize::new(0);
impl Http{fn post(&self,url:String)->Request{Request{http:self.clone(),bearer:String::new(),path:url}}}
impl Request {fn bearer_auth(mut self,v:impl AsRef<str>)->Self{self.bearer=v.as_ref().into();self}fn timeout(self,_:Duration)->Self{self}fn header(self,_:&str,_:&str)->Self{self}fn json(self,_:&Value)->Self{self}async fn send(self)->Result<Response>{let hook=HTTP_HOOK.lock().unwrap().take();if let Some(h)=hook{h()};PATHS.lock().unwrap().push(self.path);let mut http=self.http;if let Some(status)=STATUSES.lock().unwrap().pop_front(){http.status=status;}if !self.bearer.is_empty(){REQUESTS.lock().unwrap().push(self.bearer);}Ok(Response(http))}}
impl Response{fn status(&self)->Status{Status(self.0.status)}async fn text(self)->Result<String>{Ok(self.0.body.to_string())}}
#[derive(Debug)]struct RelayError(u16);
impl std::fmt::Display for RelayError{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{write!(f,"synthetic relay refusal")}}
impl std::error::Error for RelayError{}
impl RelayError{fn from_body(s:Status,_:&str)->Self{Self(s.0)}fn is_token_expired(&self)->bool{self.0==401}}

#[derive(Clone,Debug,Default)]struct Snapshot;
#[derive(Debug)]struct MessageRow;
#[derive(Deserialize)]struct Session{}
trait MatrixListener:Send+Sync {fn on_rooms(&self,snapshot:Snapshot);fn on_timeline(&self,room_id:String,rows:Vec<MessageRow>);fn on_connection(&self,connected:bool,error:Option<String>);}
struct MatrixClient{stops:AtomicUsize}
impl MatrixClient {
 async fn start(_:Session,_:Arc<dyn MatrixListener>)->Result<Arc<Self>>{SDK_CALLS.fetch_add(1,Ordering::SeqCst);let hook=MATRIX_HOOK.lock().unwrap().take();if let Some(h)=hook{h()};Ok(Arc::new(Self{stops:AtomicUsize::new(0)}))}
 fn stop(&self){self.stops.fetch_add(1,Ordering::SeqCst);SDK_STOPS.fetch_add(1,Ordering::SeqCst);}
}
fn relay_has_no_servers(_: &anyhow::Error)->bool{false}
async fn refresh_servers_list(_: &Arc<Shared>){}
struct FakeTask{aborted:Arc<AtomicBool>}impl FakeTask{fn abort(&self){self.aborted.store(true,Ordering::SeqCst)}}
struct FakeRoom{task:FakeTask}
struct FakeCallAttempt{active:Arc<AtomicBool>}impl FakeCallAttempt{fn cancel(&self){self.active.store(false,Ordering::SeqCst)}}
struct FakeSubscription{dropped:Arc<AtomicBool>}impl Drop for FakeSubscription{fn drop(&mut self){self.dropped.store(true,Ordering::SeqCst)}}
struct Queue;impl Queue{fn clear(&self){}}

use std::collections::{HashMap,HashSet};use std::path::{Path,PathBuf};use std::future::Future;
type Id=String;type ChatDoc=usize;type Project=();type Chat=();type QueuedSend=();type Machine=();type PendingMessages=();
mod reqwest{pub type Client=super::Http;}
mod loro{pub type Subscription=super::FakeSubscription;}
mod diagnostics{pub struct Diagnostics;impl Diagnostics{pub fn load()->Self{Self}}}
struct Room{task:tokio::task::AbortHandle}
struct ChatSendQueue{cleared:AtomicBool}impl ChatSendQueue{fn new(_:impl Fn(String,String)+Send+Sync+'static)->Self{Self{cleared:AtomicBool::new(false)}}fn clear(&self){self.cleared.store(true,Ordering::SeqCst);}}
#[derive(Clone,Debug,Default)]struct AccountView{id:String,login:String,name:String,avatar_url:String}
#[derive(Debug)]struct UserError;impl UserError{fn from_text(_: &str,_:&str)->Self{Self}}
#[derive(Clone,Debug)]enum ClientCommand{BindUi{control:UiControl,owner:WorkTicket},Scoped{control:WorkTicket,account:Option<WorkTicket>,command:Box<ClientCommand>},Pair{code:String},SignIn{grant_code:String},SignOut,SyntheticWrite}
#[derive(Debug)]pub enum SyncEvent{ForAccount{owner:WorkTicket,event:Box<SyncEvent>},ForCoordinator{owner:WorkTicket,event:Box<SyncEvent>},SessionStarted{control:WorkTicket,account:WorkTicket},Account(Option<AccountView>),Paired{device_id:String},NeedsPairing{relay_url:String},PairingFailed(UserError),CallAccount(WorkTicket),MessageSendFailed{chat_id:String,message_id:String,error:UserError},Pins{pins:Vec<String>,cap:usize},MatrixRooms(Snapshot),MatrixTimeline{room_id:String,rows:Vec<MessageRow>},MatrixConnection{connected:bool,error:Option<String>},ServersAvailable(bool)}
#[derive(Clone)]struct Call{call_id:String,chat_id:String}
struct CallAuthority{owner:WorkTicket,user_id:Option<String>,bearer:String}
struct CallRegistry{requests:CallRequests}impl CallRegistry{fn new(_:String)->Self{Self{requests:CallRequests::default()}}}
pub struct Store{device:String,path:PathBuf,generation:u64}impl Store{fn open(_: &str,device:&str)->Option<Self>{let path=PathBuf::from(format!("/synthetic-only/{device}"));let generation=STORE_NEXT.fetch_add(1,Ordering::SeqCst);FILES.lock().unwrap().insert(path.clone(),generation);Some(Self{device:device.into(),path,generation})}fn path(&self)->&Path{&self.path}fn put_json<T:serde::Serialize>(&self,_:&str,value:&T){WRITES.lock().unwrap().push((self.device.clone(),serde_json::to_value(value).unwrap()));}}
pub mod store{pub use super::Store;pub fn file_path(_: &str,device:&str)->std::path::PathBuf{std::path::PathBuf::from(format!("/synthetic-only/{device}"))}pub const KEY_PINS:&str="pins";pub fn remove_files(path: &std::path::Path){super::FILES.lock().unwrap().remove(path);}}
static STORE_NEXT:AtomicU64=AtomicU64::new(1);static FILES:std::sync::LazyLock<Mutex<HashMap<PathBuf,u64>>>=std::sync::LazyLock::new(||Mutex::new(HashMap::new()));static STORE_DROP_HOOK:Mutex<Option<Hook>>=Mutex::new(None);
impl Drop for Store{fn drop(&mut self){let hook=STORE_DROP_HOOK.lock().unwrap().take();if let Some(hook)=hook{hook();}}}
static PATHS:Mutex<Vec<String>>=Mutex::new(Vec::new());static WRITES:Mutex<Vec<(String,Value)>>=Mutex::new(Vec::new());static REQUESTS:Mutex<Vec<String>>=Mutex::new(Vec::new());static STATUSES:Mutex<std::collections::VecDeque<u16>>=Mutex::new(std::collections::VecDeque::new());static DISPATCH:Mutex<Vec<String>>=Mutex::new(Vec::new());
impl Http{fn get(&self,_:String)->Request{Request{http:self.clone(),bearer:String::new(),path:"synthetic-get".into()}}}
#[derive(Deserialize)]struct PinsDto{pins:Vec<String>,cap:u32}const PIN_CAP_DEFAULT:usize=5;

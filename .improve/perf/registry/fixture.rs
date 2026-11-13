use super::dto::{
    chat_from_dto, merge_chats, merged_unread, people_space_from, project_from_dto,
    relay_unread_for,
};
use crate::model::{Chat, Id, Project};
use anyhow::{anyhow, Result};
use arbos_core::api::{GetProjectResponseDto, PeopleChatDto, ProjectMembersDto, UnreadDto};
use arbos_core::wire::PROJECT_DOC_CHAT_ID;
use futures::{stream, StreamExt};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::{sync::mpsc, time::sleep};
#[path = "diagnostics.rs"]
mod diagnostics;
mod store {
    pub const KEY_ARCHIVED_CHATS: &str = "archived_chats";
}
#[derive(Default)]
struct Metrics {
    active: AtomicUsize,
    peak: AtomicUsize,
    calls: Mutex<Vec<String>>,
}
struct Active(Arc<Metrics>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}
struct Gateway {
    responses: HashMap<String, std::result::Result<Value, String>>,
    delay: u64,
    metrics: Arc<Metrics>,
}
impl Gateway {
    async fn get(&self, path: &str) -> Result<Value> {
        let n = self.metrics.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.metrics.peak.fetch_max(n, Ordering::SeqCst);
        let _active = Active(self.metrics.clone());
        self.metrics.calls.lock().unwrap().push(path.into());
        if self.delay > 0 {
            sleep(Duration::from_millis(self.delay)).await;
        }
        match self
            .responses
            .get(path)
            .unwrap_or_else(|| panic!("unplanned fixture request:{path}"))
        {
            Ok(v) => Ok(v.clone()),
            Err(e) => Err(anyhow!("{e}")),
        }
    }
}
#[derive(Default)]
struct Store(Mutex<Value>);
impl Store {
    fn put_json<T: serde::Serialize>(&self, _: &str, value: &T) {
        *self.0.lock().unwrap() = serde_json::to_value(value).unwrap();
    }
}
enum SyncEvent {
    Projects(Vec<Project>),
}
struct Shared {
    gateway: Gateway,
    http_url: String,
    projects: Mutex<Vec<Project>>,
    chats: Mutex<Vec<Chat>>,
    people: Mutex<HashSet<Id>>,
    local_read_at: Mutex<HashMap<Id, i64>>,
    local_changed_at: Mutex<HashMap<Id, u64>>,
    pending_new_chats: Mutex<HashMap<Id, ()>>,
    events: mpsc::UnboundedSender<SyncEvent>,
    store: Store,
    diagnostics: Mutex<Vec<String>>,
    rooms: Mutex<Vec<(String, String, String)>>,
}
impl Shared {
    async fn ensure_me(&self) {}
    async fn get_json(self: &Arc<Self>, path: &str) -> Result<Value> {
        self.gateway.get(path).await
    }
    fn me_id(&self) -> String {
        "gh_101".into()
    }
    fn me_handle(&self) -> String {
        "me".into()
    }
    fn token(&self) -> String {
        "synthetic".into()
    }
    fn diagnostic(&self, key: &str, error: String) {
        self.diagnostics
            .lock()
            .unwrap()
            .push(format!("{key}:{error}"));
    }
    fn with_store<T>(&self, f: impl FnOnce(&Store) -> T) -> Option<T> {
        Some(f(&self.store))
    }
}
fn now_ms() -> u64 {
    1000
}
async fn migrate_local_archive(_: &Arc<Shared>, _: &mut Vec<Chat>) {}
async fn refresh_blocks(_: &Arc<Shared>) {}
fn log_first_paint(_: &str, _: usize) {}
fn emit_chats(_: &Arc<Shared>) {}
fn apply_open_chat_hook(_: &Arc<Shared>) {}
fn save_registry(_: &Arc<Shared>) {}
fn ensure_room(s: &Arc<Shared>, pid: &str, key: &str, chat: &str) {
    s.rooms
        .lock()
        .unwrap()
        .push((pid.into(), key.into(), chat.into()));
}
include!("actual.rs");

fn fixture(n: usize, delay: u64, mixed: bool) -> (Arc<Shared>, mpsc::UnboundedReceiver<SyncEvent>) {
    let mut responses = HashMap::new();
    let mut list = Vec::new();
    let template: Value = serde_json::from_str(include_str!("project_json.json")).unwrap();
    let members: Value = serde_json::from_str(include_str!("project_members_json.json")).unwrap();
    for i in 0..n {
        let id = format!("p{i}");
        list.push(json!({"id":id,"shared":i%2==1,"owner_handle":format!("owner{i}")}));
        let mut project = template.clone();
        project["id"] = json!(id);
        project["name"] = json!(format!("Project{i}"));
        let mut chat: Value = serde_json::from_str(include_str!("chat_json.json")).unwrap();
        chat["id"] = json!(format!("chat{i}"));
        chat["project_id"] = json!(id);
        chat["archived_at"] = json!(if i == 4 { Some(42) } else { None });
        let detail = json!({"project":project,"chats":[chat],"box":"asleep","box_since":0,"spend_today":0.0,"skills_rev":""});
        responses.insert(
            format!("/projects/{id}"),
            if mixed && i == 1 {
                Err("HTTP 500".into())
            } else if mixed && i == 2 {
                Ok(json!({"invalid":"detail"}))
            } else {
                Ok(detail)
            },
        );
        let mut m = members.clone();
        m["project_id"] = json!(id);
        responses.insert(
            format!("/projects/{id}/members"),
            if mixed && i == 3 {
                Err("HTTP 404".into())
            } else {
                Ok(m)
            },
        );
    }
    if mixed {
        list.push(json!({"bad":"no-id"}));
    }
    let people: Value =
        serde_json::from_str(include_str!("people_chat_json_with_me.json")).unwrap();
    responses.insert("/projects".into(), Ok(json!(list)));
    responses.insert("/me/unread".into(),Ok(json!({"total":7,"chats":[{"project_id":"p0","chat_id":"chat0","unread":4},{"project_id":"people","chat_id":"people_dm","unread":3}]})));
    responses.insert(
        "/chats".into(),
        if mixed {
            Err("HTTP 404".into())
        } else {
            Ok(json!([people.clone()]))
        },
    );
    let (tx, rx) = mpsc::unbounded_channel();
    let (mut cached_people, cached_chat) = people_space_from(
        &serde_json::from_value::<PeopleChatDto>(people).unwrap(),
        "gh_101",
        "me",
        "http://fixture.invalid",
        "synthetic",
    );
    cached_people.name = "cached-DM".into();
    let mut previous = if mixed {
        vec![Project {
            id: "p1".into(),
            name: "cached-one".into(),
            muted: true,
            ..Project::default()
        }]
    } else {
        Vec::new()
    };
    previous.push(cached_people);
    let mut local: Value = serde_json::from_str(include_str!("chat_json.json")).unwrap();
    local["id"] = json!("chat0");
    local["project_id"] = json!("p0");
    local["title"] = json!("locally edited");
    let local = chat_from_dto(&serde_json::from_value(local).unwrap());
    let mut previous_chats = vec![local, cached_chat];
    if mixed {
        let mut c = previous_chats[0].clone();
        c.id = "cached-chat1".into();
        c.project_id = "p1".into();
        previous_chats.push(c);
    }
    let s = Shared {
        gateway: Gateway {
            responses,
            delay,
            metrics: Arc::new(Metrics::default()),
        },
        http_url: "http://fixture.invalid".into(),
        projects: Mutex::new(previous),
        chats: Mutex::new(previous_chats),
        people: Mutex::new(HashSet::from(["people_dm".into()])),
        local_read_at: Mutex::new(HashMap::from([("chat0".into(), i64::MAX)])),
        local_changed_at: Mutex::new(HashMap::from([("chat0".into(), 2000)])),
        pending_new_chats: Mutex::new(HashMap::new()),
        events: tx,
        store: Store::default(),
        diagnostics: Mutex::new(Vec::new()),
        rooms: Mutex::new(Vec::new()),
    };
    (Arc::new(s), rx)
}
pub async fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|v| v == "cancel") {
        let (s, mut rx) = fixture(12, 20, false);
        let before_projects = s.projects.lock().unwrap().clone();
        let before_chats = s.chats.lock().unwrap().clone();
        assert!(
            tokio::time::timeout(Duration::from_millis(35), refresh_registry(&s))
                .await
                .is_err()
        );
        assert_eq!(*s.projects.lock().unwrap(), before_projects);
        assert_eq!(*s.chats.lock().unwrap(), before_chats);
        assert!(rx.try_recv().is_err());
        assert!(s.rooms.lock().unwrap().is_empty());
        assert_eq!(s.gateway.metrics.active.load(Ordering::SeqCst), 0);
        println!(
            "{}",
            json!({"cancelled":true,"no_publication":true,"active":0,"peak":s.gateway.metrics.peak.load(Ordering::SeqCst)})
        );
        return Ok(());
    }
    let bench = args.get(1).is_some_and(|v| v == "bench");
    let n = if bench { 12 } else { 6 };
    let delay = if bench {
        args.get(2).map(|s| s.parse().unwrap()).unwrap_or(8)
    } else {
        0
    };
    let mut outputs = Vec::new();
    for mixed in if bench {
        vec![false]
    } else {
        vec![false, true]
    } {
        let (s, mut rx) = fixture(n, delay, mixed);
        let start = Instant::now();
        refresh_registry(&s).await?;
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        let projects = s.projects.lock().unwrap().clone();
        let chats = s.chats.lock().unwrap().clone();
        assert_eq!(projects.len(), if mixed { n } else { n + 1 });
        assert_eq!(
            chats.iter().find(|c| c.id == "chat0").unwrap().title,
            "locally edited"
        );
        assert_eq!(
            chats.iter().find(|c| c.id == "chat0").unwrap().unread_count,
            0
        );
        assert_eq!(
            chats
                .iter()
                .find(|c| c.id == "people_dm")
                .unwrap()
                .unread_count,
            3
        );
        assert_eq!(
            chats.iter().find(|c| c.id == "chat4").unwrap().archived_at,
            Some(42)
        );
        assert_eq!(
            projects.last().unwrap().name,
            if mixed { "cached-DM" } else { "user102" }
        );
        assert_eq!(projects[0].id, "p0");
        if mixed {
            assert_eq!(projects[1].name, "cached-one");
            assert!(projects[1].muted);
            assert!(!projects.iter().any(|p| p.id == "p2"));
            assert!(projects
                .iter()
                .find(|p| p.id == "p3")
                .unwrap()
                .people
                .is_empty());
        }
        let mut emitted = 0;
        while let Ok(SyncEvent::Projects(p)) = rx.try_recv() {
            assert_eq!(p, projects);
            emitted += 1;
        }
        assert_eq!(emitted, 1);
        assert_eq!(s.gateway.metrics.active.load(Ordering::SeqCst), 0);
        let mut calls = s.gateway.metrics.calls.lock().unwrap().clone();
        calls.sort();
        outputs.push(json!({"mixed":mixed,"projects":projects,"chats":chats,"calls":calls,"rooms":s.rooms.lock().unwrap().clone(),"peak":s.gateway.metrics.peak.load(Ordering::SeqCst),"milliseconds":elapsed}));
    }
    println!(
        "{}",
        json!({"cases":outputs,"mode":if bench{"controlled-8ms-gateway-benchmark"}else{"actual-registry-controls"}})
    );
    Ok(())
}

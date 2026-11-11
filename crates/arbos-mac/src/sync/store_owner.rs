//! One process-wide owner per actual cache file, including source reincarnation.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use super::store::{self, Store};
use super::work_lifetime::WorkTicket;

#[derive(Clone, PartialEq, Eq)]
struct Origin {
    relay: String,
    device: String,
}

enum State {
    Empty,
    Open {
        owner: WorkTicket,
        store: Store,
    },
    Retiring {
        owner: WorkTicket,
        store: Store,
        wipe: bool,
    },
}

struct Slot {
    path: PathBuf,
    state: Mutex<State>,
}

pub(crate) struct StoreLease {
    slot: Option<Arc<Slot>>,
    origin: Origin,
    owner: WorkTicket,
}

pub(crate) struct StoreRetirement {
    slot: Arc<Slot>,
    owner: WorkTicket,
}

// Generated basenames contain ASCII only. Lowercasing conservatively serializes
// case aliases on macOS; exact origins still decide whether cached data may be read.
fn file_key(path: &std::path::Path) -> PathBuf {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut parent = path.parent().unwrap_or(std::path::Path::new("/"));
    let mut missing = Vec::new();
    let mut key = loop {
        if let Ok(existing) = std::fs::canonicalize(parent) {
            break existing;
        }
        match (parent.file_name(), parent.parent()) {
            (Some(name), Some(ancestor)) => {
                missing.push(name.to_os_string());
                parent = ancestor;
            }
            _ => break parent.to_path_buf(),
        }
    };
    for component in missing.into_iter().rev() {
        key.push(component);
    }
    if let Some(name) = path.file_name() {
        key.push(
            name.to_str()
                .map(str::to_ascii_lowercase)
                .map(std::ffi::OsString::from)
                .unwrap_or_else(|| name.to_os_string()),
        );
    }
    key
}

struct Entry {
    origin: Origin,
    user_id: Option<String>,
    slot: Weak<Slot>,
}

fn slots() -> &'static Mutex<HashMap<PathBuf, Entry>> {
    static SLOTS: OnceLock<Mutex<HashMap<PathBuf, Entry>>> = OnceLock::new();
    SLOTS.get_or_init(Default::default)
}

impl StoreLease {
    pub(crate) fn new(relay: &str, device: &str, user_id: Option<&str>, owner: WorkTicket) -> Self {
        let path = store::file_path(relay, device);
        let key = file_key(&path);
        let origin = Origin {
            relay: relay.to_string(),
            device: device.to_string(),
        };
        let slot = {
            let mut slots = slots().lock().expect("store file owners");
            let entry = slots.entry(key).or_insert_with(|| Entry {
                origin: origin.clone(),
                user_id: None,
                slot: Weak::new(),
            });
            // Keep lightweight origin metadata even after the last SQLite lease
            // ends: a later lossy filename collision is still a different origin.
            if entry.origin != origin
                || matches!((&entry.user_id, user_id), (Some(before), Some(now)) if before != now)
            {
                None
            } else {
                if entry.user_id.is_none() {
                    entry.user_id = user_id.map(str::to_owned);
                }
                let slot = entry.slot.upgrade().unwrap_or_else(|| {
                    let slot = Arc::new(Slot {
                        path: path.clone(),
                        state: Mutex::new(State::Empty),
                    });
                    entry.slot = Arc::downgrade(&slot);
                    slot
                });
                Some(slot)
            }
        };
        Self {
            slot,
            origin,
            owner,
        }
    }

    pub(crate) fn open(&self) -> bool {
        self.open_with(|| Store::open(&self.origin.relay, &self.origin.device))
    }

    fn open_with(&self, open: impl FnOnce() -> Option<Store>) -> bool {
        // A lossy legacy filename is not proof that two different origins share data.
        // This in-process fence does not attribute an unbound legacy cache.
        let Some(slot) = &self.slot else { return false };
        let mut state = slot.state.lock().expect("store file owner");
        if matches!(*state, State::Retiring { .. }) {
            finish_retiring(&mut state, &slot.path);
        }
        match &*state {
            State::Open { owner, .. } => *owner == self.owner,
            State::Empty => {
                if let Some(store) = open() {
                    if file_key(store.path()) != file_key(&slot.path) {
                        return false;
                    }
                    *state = State::Open {
                        owner: self.owner,
                        store,
                    };
                    true
                } else {
                    false
                }
            }
            State::Retiring { .. } => unreachable!("retirement completed before open"),
        }
    }

    pub(crate) fn with<R>(&self, f: impl FnOnce(&Store) -> R) -> Option<R> {
        let slot = self.slot.as_ref()?;
        let state = slot.state.lock().expect("store file owner");
        match &*state {
            State::Open { owner, store } if *owner == self.owner => Some(f(store)),
            _ => None,
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.with(|_| ()).is_some()
    }

    /// Revoke reads/writes under the account fence without closing SQLite there.
    pub(crate) fn retire(&self, wipe: bool) -> Option<StoreRetirement> {
        let slot = self.slot.as_ref()?;
        let mut state = slot.state.lock().expect("store file owner");
        let old = std::mem::replace(&mut *state, State::Empty);
        match old {
            State::Open { owner, store } if owner == self.owner => {
                *state = State::Retiring { owner, store, wipe };
            }
            State::Retiring {
                owner,
                store,
                wipe: before,
            } if owner == self.owner => {
                *state = State::Retiring {
                    owner,
                    store,
                    wipe: before || wipe,
                };
            }
            other => {
                *state = other;
                return None;
            }
        }
        Some(StoreRetirement {
            slot: slot.clone(),
            owner: self.owner,
        })
    }
}

impl StoreRetirement {
    pub(crate) fn finish(self) {
        let mut state = self.slot.state.lock().expect("store file owner");
        if matches!(*state, State::Retiring { owner, .. } if owner == self.owner) {
            finish_retiring(&mut state, &self.slot.path);
        }
    }
}

/// The per-file guard spans close and unlink, and is never an account/SDK/doc guard.
fn finish_retiring(state: &mut State, path: &std::path::Path) {
    if let State::Retiring { store, wipe, .. } = std::mem::replace(state, State::Empty) {
        drop(store);
        if wipe {
            store::remove_files(path);
        }
    }
}

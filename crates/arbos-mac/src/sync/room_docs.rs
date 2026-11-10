use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

pub(crate) struct RoomDoc<T> {
    value: Mutex<T>,
    retired: AtomicBool,
}

impl<T> RoomDoc<T> {
    pub(crate) fn is_retired(&self) -> bool {
        self.retired.load(Ordering::Acquire)
    }

    pub(crate) fn lock(&self) -> Option<MutexGuard<'_, T>> {
        if self.is_retired() {
            return None;
        }
        let value = self.value.lock().unwrap();
        (!self.is_retired()).then_some(value)
    }
}

pub(crate) struct RoomDocs<T> {
    entries: Mutex<HashMap<String, Arc<RoomDoc<T>>>>,
}

impl<T> Default for RoomDocs<T> {
    fn default() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }
}

impl<T> Drop for RoomDocs<T> {
    fn drop(&mut self) {
        drop(self.retire_all());
    }
}

impl<T> RoomDocs<T> {
    pub(crate) fn get(&self, key: &str) -> Option<Arc<RoomDoc<T>>> {
        self.entries.lock().unwrap().get(key).cloned()
    }

    pub(crate) fn contains_key(&self, key: &str) -> bool {
        self.entries.lock().unwrap().contains_key(key)
    }

    pub(crate) fn insert(&self, key: String, doc: T) -> Arc<RoomDoc<T>> {
        let doc = Arc::new(RoomDoc {
            value: Mutex::new(doc),
            retired: AtomicBool::new(false),
        });
        let previous = {
            let mut entries = self.entries.lock().unwrap();
            if let Some(previous) = entries.get(&key) {
                previous.retired.store(true, Ordering::Release);
            }
            entries.insert(key, doc.clone())
        };
        drop(previous);
        doc
    }

    /// Drop the returned resources after releasing account/registry ownership locks.
    pub(crate) fn retire_all(&self) -> Vec<Arc<RoomDoc<T>>> {
        // Teardown may revoke poisoned work; admission still fails closed on poison.
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        for doc in entries.values() {
            doc.retired.store(true, Ordering::Release);
        }
        entries.drain().map(|(_, doc)| doc).collect()
    }

    /// The publisher must be short and nonblocking, and must not acquire a document lock.
    pub(crate) fn publish_if_current<R>(
        &self,
        key: &str,
        doc: &Arc<RoomDoc<T>>,
        publish: impl FnOnce() -> R,
    ) -> Option<R> {
        let entries = self.entries.lock().unwrap();
        entries
            .get(key)
            .filter(|current| Arc::ptr_eq(current, doc))
            .map(|_| publish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn paused_room_work_does_not_hold_another_room_or_the_registry() {
        let docs = Arc::new(RoomDocs::default());
        docs.insert("a".into(), 1);
        docs.insert("b".into(), 2);
        let a = docs.get("a").unwrap();
        let paused = a.lock().unwrap();
        let (done, received) = mpsc::channel();
        let reader = docs.clone();
        let task = std::thread::spawn(move || {
            let b = reader.get("b").unwrap();
            let value = *b.lock().unwrap();
            done.send((value, reader.contains_key("a"))).unwrap();
        });
        let result = received.recv_timeout(Duration::from_secs(2));
        drop(paused);
        task.join().unwrap();
        assert_eq!(result.unwrap(), (2, true));
    }

    #[test]
    fn replaced_or_different_room_cannot_publish_an_old_read() {
        let docs = RoomDocs::default();
        docs.insert("a".into(), 1);
        let old = docs.get("a").unwrap();
        let old_read = old.lock().unwrap();
        docs.insert("a".into(), 2);
        drop(old_read);
        assert!(old.lock().is_none());

        let mut events = Vec::new();
        assert!(docs
            .publish_if_current("a", &old, || events.push(1))
            .is_none());
        let current = docs.get("a").unwrap();
        assert!(docs
            .publish_if_current("b", &current, || events.push(3))
            .is_none());
        assert!(docs
            .publish_if_current("a", &current, || events.push(2))
            .is_some());
        assert_eq!(events, [2]);
    }

    #[test]
    fn account_retirement_rejects_waiting_work_and_old_publication() {
        let docs = RoomDocs::default();
        let old = docs.insert("a".into(), 1);
        let active = old.lock().unwrap();
        let retired = docs.retire_all();
        assert!(docs.get("a").is_none());
        let current = docs.insert("a".into(), 2);
        let mut events = Vec::new();
        assert!(docs
            .publish_if_current("a", &old, || events.push(1))
            .is_none());
        drop(active);
        assert!(old.lock().is_none());
        assert_eq!(*current.lock().unwrap(), 2);
        assert!(docs
            .publish_if_current("a", &current, || events.push(2))
            .is_some());
        assert_eq!(events, [2]);
        drop(retired);
    }

    #[test]
    fn an_already_retired_reader_does_not_wait_for_an_active_old_read() {
        let docs = RoomDocs::default();
        let old = docs.insert("a".into(), 1);
        let active = old.lock().unwrap();
        let retired = docs.retire_all();
        let reader = old.clone();
        let (done, received) = mpsc::channel();
        let task = std::thread::spawn(move || done.send(reader.lock().is_none()).unwrap());
        let result = received.recv_timeout(Duration::from_secs(2));
        drop(active);
        task.join().unwrap();
        assert!(result.unwrap());
        drop(retired);
    }

    #[test]
    fn a_contending_reader_rechecks_retirement_after_acquiring_the_document() {
        for _ in 0..16 {
            let docs = RoomDocs::default();
            let old = docs.insert("a".into(), 1);
            let active = old.lock().unwrap();
            let reader = old.clone();
            let (started, ready) = mpsc::channel();
            let task = std::thread::spawn(move || {
                started.send(()).unwrap();
                reader.lock().is_none()
            });
            ready.recv_timeout(Duration::from_secs(2)).unwrap();
            for _ in 0..128 {
                std::thread::yield_now();
            }
            let retired = docs.retire_all();
            drop(active);
            assert!(task.join().unwrap());
            drop(retired);
        }
    }

    #[test]
    fn registry_drop_retires_outstanding_document_handles() {
        let docs = RoomDocs::default();
        let handle = docs.insert("a".into(), 1);
        drop(docs);
        assert!(handle.lock().is_none());
    }

    #[test]
    fn registry_drop_retires_handles_after_a_publisher_panics() {
        let docs = RoomDocs::default();
        let handle = docs.insert("a".into(), 1);
        let failed = std::panic::catch_unwind(|| {
            docs.publish_if_current("a", &handle, || panic!("synthetic publisher failure"));
        });
        assert!(failed.is_err());
        drop(docs);
        assert!(handle.lock().is_none());
    }
}

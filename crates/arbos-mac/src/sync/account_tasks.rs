//! One account's ordinary tasks. Retirement closes admission before abort/drain.
use std::future::Future;
use std::sync::Mutex;
use tokio::task::{AbortHandle, JoinSet};

enum Tasks {
    Open(JoinSet<()>),
    Closed,
}

pub(crate) struct TaskOwner {
    tasks: Mutex<Tasks>,
}

impl Default for TaskOwner {
    fn default() -> Self {
        Self {
            tasks: Mutex::new(Tasks::Open(JoinSet::new())),
        }
    }
}

impl TaskOwner {
    pub(crate) fn spawn(
        &self,
        future: impl Future<Output = ()> + Send + 'static,
    ) -> Option<AbortHandle> {
        let Ok(mut state) = self.tasks.lock() else {
            return None;
        };
        match &mut *state {
            Tasks::Open(tasks) => {
                // Completed tasks release their bookkeeping on the next admission.
                while tasks.try_join_next().is_some() {}
                Some(tasks.spawn(future))
            }
            Tasks::Closed => {
                drop(state);
                drop(future);
                None
            }
        }
    }

    /// The caller awaits/drops the detached set outside account/resource guards.
    pub(crate) fn retire(&self) -> JoinSet<()> {
        let mut state = self
            .tasks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let old = std::mem::replace(&mut *state, Tasks::Closed);
        drop(state);
        match old {
            Tasks::Open(mut tasks) => {
                tasks.abort_all();
                tasks
            }
            Tasks::Closed => JoinSet::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Weak,
    };
    use tokio::sync::Notify;

    struct Resource {
        owner: Weak<TaskOwner>,
        drops: Arc<AtomicUsize>,
        outside: Arc<AtomicBool>,
    }
    impl Drop for Resource {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
            self.outside.store(
                self.owner
                    .upgrade()
                    .is_none_or(|owner| owner.tasks.try_lock().is_ok()),
                Ordering::SeqCst,
            );
        }
    }

    #[tokio::test]
    async fn retirement_aborts_drains_once_and_rejects_late_work() {
        let owner = Arc::new(TaskOwner::default());
        let began = Arc::new(Notify::new());
        let drops = Arc::new(AtomicUsize::new(0));
        let outside = Arc::new(AtomicBool::new(false));
        let resource = Resource {
            owner: Arc::downgrade(&owner),
            drops: drops.clone(),
            outside: outside.clone(),
        };
        let signal = began.clone();
        assert!(owner
            .spawn(async move {
                let _owned = resource;
                signal.notify_one();
                std::future::pending::<()>().await;
            })
            .is_some());
        began.notified().await;
        let mut retired = owner.retire();
        assert_eq!(retired.len(), 1);
        let mut joined = 0;
        while let Some(result) = retired.join_next().await {
            assert!(result.unwrap_err().is_cancelled());
            joined += 1;
        }
        assert_eq!(joined, 1);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert!(outside.load(Ordering::SeqCst));
        assert!(owner.retire().is_empty());
        let ran = Arc::new(AtomicBool::new(false));
        let flag = ran.clone();
        assert!(owner
            .spawn(async move {
                flag.store(true, Ordering::SeqCst);
            })
            .is_none());
        tokio::task::yield_now().await;
        assert!(!ran.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn current_work_runs_and_completed_tasks_are_reaped() {
        let owner = TaskOwner::default();
        let ran = Arc::new(AtomicUsize::new(0));
        for _ in 0..4 {
            let ran = ran.clone();
            assert!(owner
                .spawn(async move {
                    ran.fetch_add(1, Ordering::SeqCst);
                })
                .is_some());
            tokio::task::yield_now().await;
        }
        assert_eq!(ran.load(Ordering::SeqCst), 4);
        let tasks = owner.retire();
        assert!(tasks.len() <= 1);
    }

    #[tokio::test]
    async fn rejected_resource_is_destroyed_outside_owner_lock() {
        let owner = Arc::new(TaskOwner::default());
        drop(owner.retire());
        let drops = Arc::new(AtomicUsize::new(0));
        let outside = Arc::new(AtomicBool::new(false));
        let resource = Resource {
            owner: Arc::downgrade(&owner),
            drops: drops.clone(),
            outside: outside.clone(),
        };
        assert!(owner
            .spawn(async move {
                drop(resource);
            })
            .is_none());
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert!(outside.load(Ordering::SeqCst));
    }
}

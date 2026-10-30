//! One ordered send queue per chat.
//!
//! Sends that used to `tokio::spawn` raced when a photo upload was slower than
//! a following text. Jobs are pushed synchronously; a single worker per chat
//! runs them one after another. Other chats are unaffected.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::model::Id;

/// Generous enough for a large attachment upload.
pub const SEND_JOB_TIMEOUT: Duration = Duration::from_secs(5 * 60);

type JobFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
type OnTimeout = Arc<dyn Fn(Id, Id) + Send + Sync>;

struct Job {
    message_id: Id,
    fut: JobFuture,
}

struct ChatEntry {
    tx: mpsc::UnboundedSender<Job>,
    worker: JoinHandle<()>,
}

/// Per-chat unbounded queue: push order is send order.
pub struct ChatSendQueue {
    chats: Mutex<HashMap<Id, ChatEntry>>,
    timeout: Duration,
    on_timeout: OnTimeout,
}

impl ChatSendQueue {
    pub fn new(on_timeout: impl Fn(Id, Id) + Send + Sync + 'static) -> Self {
        Self {
            chats: Mutex::new(HashMap::new()),
            timeout: SEND_JOB_TIMEOUT,
            on_timeout: Arc::new(on_timeout),
        }
    }

    /// Bound used by tests; production keeps [`SEND_JOB_TIMEOUT`].
    #[cfg(test)]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Enqueue a send for `chat_id`. Order is fixed here, not when the worker runs.
    pub fn push<F>(&self, chat_id: Id, message_id: Id, fut: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut chats = self.chats.lock().unwrap();
        let tx = match chats.get(&chat_id) {
            Some(entry) => entry.tx.clone(),
            None => {
                let (tx, rx) = mpsc::unbounded_channel();
                let job_timeout = self.timeout;
                let on_timeout = Arc::clone(&self.on_timeout);
                let worker_chat = chat_id.clone();
                let worker = tokio::spawn(async move {
                    run_worker(worker_chat, rx, job_timeout, on_timeout).await;
                });
                chats.insert(
                    chat_id.clone(),
                    ChatEntry {
                        tx: tx.clone(),
                        worker,
                    },
                );
                tx
            }
        };
        let _ = tx.send(Job {
            message_id,
            fut: Box::pin(fut),
        });
    }

    /// Abort every worker and drop the channels (sign-out).
    pub fn clear(&self) {
        let mut chats = self.chats.lock().unwrap();
        for (_, entry) in chats.drain() {
            entry.worker.abort();
        }
    }
}

async fn run_worker(
    chat_id: Id,
    mut rx: mpsc::UnboundedReceiver<Job>,
    job_timeout: Duration,
    on_timeout: OnTimeout,
) {
    while let Some(job) = rx.recv().await {
        match timeout(job_timeout, job.fut).await {
            Ok(()) => {}
            Err(_) => on_timeout(chat_id.clone(), job.message_id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::sync::oneshot;
    use tokio::time::sleep;

    #[tokio::test]
    async fn one_chat_runs_in_push_order_even_when_first_is_slower() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let q = ChatSendQueue::new(|_, _| {}).with_timeout(Duration::from_secs(30));
        let (done_tx, done_rx) = oneshot::channel();

        let order1 = Arc::clone(&order);
        q.push("chat-a".into(), "m1".into(), async move {
            sleep(Duration::from_millis(80)).await;
            order1.lock().unwrap().push(1);
        });
        let order2 = Arc::clone(&order);
        q.push("chat-a".into(), "m2".into(), async move {
            order2.lock().unwrap().push(2);
            let _ = done_tx.send(());
        });

        done_rx.await.unwrap();
        assert_eq!(*order.lock().unwrap(), vec![1, 2]);
    }

    #[tokio::test]
    async fn different_chats_run_independently() {
        let q = ChatSendQueue::new(|_, _| {}).with_timeout(Duration::from_secs(30));
        let (fast_tx, fast_rx) = oneshot::channel();
        let slow_started = Arc::new(AtomicBool::new(false));
        let slow_flag = Arc::clone(&slow_started);

        q.push("slow".into(), "m1".into(), async move {
            slow_flag.store(true, Ordering::SeqCst);
            sleep(Duration::from_millis(200)).await;
        });
        q.push("fast".into(), "m2".into(), async move {
            // Wait until the slow chat's job has actually started, then finish
            // without waiting for it — proves we are not blocked by the other chat.
            while !slow_started.load(Ordering::SeqCst) {
                sleep(Duration::from_millis(5)).await;
            }
            let _ = fast_tx.send(());
        });

        tokio::time::timeout(Duration::from_millis(100), fast_rx)
            .await
            .expect("fast chat should not wait on the slow chat")
            .unwrap();
    }

    #[tokio::test]
    async fn early_failure_does_not_block_next_job() {
        let ran_second = Arc::new(AtomicBool::new(false));
        let q = ChatSendQueue::new(|_, _| {}).with_timeout(Duration::from_secs(30));
        let (done_tx, done_rx) = oneshot::channel();

        q.push("chat-a".into(), "m1".into(), async move {
            // Simulate send_message returning after a notice / MessageSendFailed.
        });
        let flag = Arc::clone(&ran_second);
        q.push("chat-a".into(), "m2".into(), async move {
            flag.store(true, Ordering::SeqCst);
            let _ = done_tx.send(());
        });

        done_rx.await.unwrap();
        assert!(ran_second.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn stuck_job_is_cut_at_timeout_and_next_runs() {
        let timed_out = Arc::new(AtomicUsize::new(0));
        let timed_out_cb = Arc::clone(&timed_out);
        let q = ChatSendQueue::new(move |_chat, _msg| {
            timed_out_cb.fetch_add(1, Ordering::SeqCst);
        })
        .with_timeout(Duration::from_millis(40));
        let (done_tx, done_rx) = oneshot::channel();

        q.push("chat-a".into(), "m1".into(), async move {
            sleep(Duration::from_secs(30)).await;
        });
        q.push("chat-a".into(), "m2".into(), async move {
            let _ = done_tx.send(());
        });

        tokio::time::timeout(Duration::from_secs(2), done_rx)
            .await
            .expect("second job should run after the timeout")
            .unwrap();
        assert_eq!(timed_out.load(Ordering::SeqCst), 1);
    }
}

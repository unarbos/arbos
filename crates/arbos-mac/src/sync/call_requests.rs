//! Reserve call HTTP order at admission, before a spawned future is first polled.
use std::sync::Mutex;

use futures::future::{BoxFuture, Shared};
use futures::FutureExt;
use tokio::sync::oneshot;

type Completion = Shared<BoxFuture<'static, ()>>;

#[derive(Default)]
pub(crate) struct CallRequests {
    tail: Mutex<Option<Completion>>,
}

pub(crate) struct CallRequestTurn {
    previous: Option<Completion>,
    finished: Option<oneshot::Sender<()>>,
}

impl CallRequests {
    pub(crate) fn reserve(&self) -> CallRequestTurn {
        let mut tail = self.tail.lock().expect("call request order");
        let previous = tail.clone();
        let predecessor = previous.clone();
        let (finished, completion) = oneshot::channel();
        // Even if this waiter is cancelled, its successor must still wait for
        // the earlier owner. No request payload or authority enters this chain.
        *tail = Some(
            async move {
                if let Some(previous) = predecessor {
                    previous.await;
                }
                let _ = completion.await;
            }
            .boxed()
            .shared(),
        );
        CallRequestTurn {
            previous,
            finished: Some(finished),
        }
    }
}

impl CallRequestTurn {
    pub(crate) async fn enter(mut self) -> Self {
        if let Some(previous) = self.previous.take() {
            previous.await;
        }
        self
    }
}

impl Drop for CallRequestTurn {
    fn drop(&mut self) {
        if let Some(finished) = self.finished.take() {
            let _ = finished.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    #[tokio::test]
    async fn reservation_order_survives_reverse_poll_order() {
        let requests = CallRequests::default();
        let first = requests.reserve();
        let second = requests.reserve();
        let third = requests.reserve();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let third_seen = seen.clone();
        let third = tokio::spawn(async move {
            let _turn = third.enter().await;
            third_seen.lock().unwrap().push(3);
        });
        let second_seen = seen.clone();
        let second = tokio::spawn(async move {
            let _turn = second.enter().await;
            second_seen.lock().unwrap().push(2);
        });
        tokio::task::yield_now().await;
        assert!(seen.lock().unwrap().is_empty());
        let first = first.enter().await;
        seen.lock().unwrap().push(1);
        drop(first);
        second.await.unwrap();
        third.await.unwrap();
        assert_eq!(*seen.lock().unwrap(), [1, 2, 3]);
    }

    #[tokio::test]
    async fn dropping_unpolled_waiter_does_not_bypass_earlier_owner() {
        let requests = CallRequests::default();
        let first = requests.reserve().enter().await;
        let skipped = requests.reserve();
        let last = requests.reserve();
        drop(skipped);
        let entered = Arc::new(AtomicBool::new(false));
        let flag = entered.clone();
        let last = tokio::spawn(async move {
            let _turn = last.enter().await;
            flag.store(true, Ordering::SeqCst);
        });
        tokio::task::yield_now().await;
        assert!(!entered.load(Ordering::SeqCst));
        drop(first);
        last.await.unwrap();
        assert!(entered.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn aborting_waiter_does_not_bypass_earlier_owner() {
        let requests = CallRequests::default();
        let first = requests.reserve().enter().await;
        let middle = requests.reserve();
        let last = requests.reserve();
        let middle = tokio::spawn(async move {
            let _turn = middle.enter().await;
        });
        tokio::task::yield_now().await;
        middle.abort();
        assert!(middle.await.unwrap_err().is_cancelled());
        let entered = Arc::new(AtomicBool::new(false));
        let flag = entered.clone();
        let last = tokio::spawn(async move {
            let _turn = last.enter().await;
            flag.store(true, Ordering::SeqCst);
        });
        tokio::task::yield_now().await;
        assert!(!entered.load(Ordering::SeqCst));
        drop(first);
        last.await.unwrap();
        assert!(entered.load(Ordering::SeqCst));
    }
}

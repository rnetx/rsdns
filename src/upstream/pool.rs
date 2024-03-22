use std::{collections::VecDeque, sync::Arc, time::Duration};

use chrono::{DateTime, Local};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::Mutex,
};

use crate::common;

struct PoolItem<T> {
    item: T,
    expired: DateTime<Local>,
}

const DEFAULT_MAX_SIZE: usize = 16;
const DEFAULT_CLEAN_INTERVAL: Duration = Duration::from_secs(5);

pub(super) struct Pool<T> {
    queue: Arc<Mutex<VecDeque<PoolItem<T>>>>,
    idle_timeout: Duration,
    canceller: Option<common::Canceller>,
}

impl<T: Send + 'static> Pool<T> {
    pub(super) async fn new_pool(mut max_size: usize, idle_timeout: Duration) -> Self {
        if max_size == 0 {
            max_size = DEFAULT_MAX_SIZE;
        }
        let queue = Arc::new(Mutex::new(VecDeque::with_capacity(max_size)));
        let queue_cloned = queue.clone();
        let (canceller, canceller_guard) = common::new_canceller();
        tokio::spawn(async move {
            Self::clean_handle(queue_cloned, canceller_guard).await;
        });
        Self {
            queue,
            idle_timeout,
            canceller: Some(canceller),
        }
    }

    async fn clean_handle(
        queue: Arc<Mutex<VecDeque<PoolItem<T>>>>,
        canceller_guard: common::CancellerGuard,
    ) {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(DEFAULT_CLEAN_INTERVAL) => {
                    let mut queue = queue.lock().await;
                    let queue_length = queue.len();
                    let now = Local::now();
                    for _ in 0..queue_length {
                        if let Some(item) = queue.pop_front() {
                            if item.expired > now {
                                queue.push_back(item);
                            }
                        } else {
                            break;
                        }
                    }
                }
                _ = canceller_guard.cancelled() => {
                    break;
                }
            }
        }
    }

    pub(super) async fn close(&mut self) {
        if let Some(mut canceller) = self.canceller.take() {
            canceller.cancel_and_wait().await;
        }
        while let Some(_) = self.queue.lock().await.pop_front() {}
    }

    pub(super) async fn get(&self) -> Option<T> {
        let mut queue = self.queue.lock().await;
        while let Some(item) = queue.pop_front() {
            if item.expired > Local::now() {
                return Some(item.item);
            }
        }
        None
    }

    pub(super) async fn put(&self, item: T) {
        let mut queue = self.queue.lock().await;
        if queue.len() < queue.capacity() {
            queue.push_back(PoolItem {
                item,
                expired: Local::now() + self.idle_timeout,
            });
        }
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static> Pool<super::PipelineStream<S>> {
    pub(super) async fn new_pipeline_stream_pool(
        mut max_size: usize,
        idle_timeout: Duration,
    ) -> Self {
        if max_size == 0 {
            max_size = DEFAULT_MAX_SIZE;
        }
        let queue = Arc::new(Mutex::new(VecDeque::with_capacity(max_size)));
        let queue_cloned = queue.clone();
        let (canceller, canceller_guard) = common::new_canceller();
        tokio::spawn(async move {
            Self::clean_pipeline_stream_handle(queue_cloned, canceller_guard).await;
        });
        Self {
            queue,
            idle_timeout,
            canceller: Some(canceller),
        }
    }

    async fn clean_pipeline_stream_handle(
        queue: Arc<Mutex<VecDeque<PoolItem<super::PipelineStream<S>>>>>,
        canceller_guard: common::CancellerGuard,
    ) {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(DEFAULT_CLEAN_INTERVAL) => {
                    let mut queue = queue.lock().await;
                    let queue_length = queue.len();
                    let now = Local::now();
                    for _ in 0..queue_length {
                        if let Some(item) = queue.pop_front() {
                            if item.expired > now && !item.item.is_cancelled() {
                                queue.push_back(item);
                            }
                        } else {
                            break;
                        }
                    }
                }
                _ = canceller_guard.cancelled() => {
                    break;
                }
            }
        }
    }

    pub(super) async fn get_pipeline_stream(&self) -> Option<super::PipelineStream<S>> {
        let mut queue = self.queue.lock().await;
        let now = Local::now();
        while let Some(item) = queue.pop_front() {
            if item.expired > now && !item.item.is_cancelled() {
                let item_cloned = item.item.clone();
                queue.push_back(item);
                return Some(item_cloned);
            }
        }
        None
    }
}

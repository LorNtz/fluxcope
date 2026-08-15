use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use anyhow::{Context, Result};
use tokio::{sync::watch, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use super::{SearchJobOutcome, SearchRequest, run_search};

trait SearchRunner: Clone + Send + Sync + 'static {
    fn run(&self, request: &SearchRequest, cancelled: &AtomicBool) -> SearchJobOutcome;
}

#[derive(Clone, Copy)]
struct MatcherRunner;

impl SearchRunner for MatcherRunner {
    fn run(&self, request: &SearchRequest, cancelled: &AtomicBool) -> SearchJobOutcome {
        run_search(request, cancelled)
    }
}

#[derive(Clone)]
pub(crate) struct RequestSearchClient {
    requests: watch::Sender<Option<SearchRequest>>,
}

impl RequestSearchClient {
    pub(crate) fn submit(&self, request: SearchRequest) {
        self.requests.send_replace(Some(request));
    }

    pub(crate) fn cancel(&self) {
        self.requests.send_replace(None);
    }
}

pub(crate) struct RequestSearchService {
    pub client: RequestSearchClient,
    pub results: watch::Receiver<Option<Arc<SearchJobOutcome>>>,
    pub task: JoinHandle<Result<()>>,
}

pub(crate) fn start_request_search_service(shutdown: CancellationToken) -> RequestSearchService {
    let (request_tx, request_rx) = watch::channel(None);
    let (result_tx, result_rx) = watch::channel(None);
    let task = tokio::spawn(run_service(request_rx, result_tx, shutdown, MatcherRunner));
    RequestSearchService {
        client: RequestSearchClient {
            requests: request_tx,
        },
        results: result_rx,
        task,
    }
}

async fn run_service<R: SearchRunner>(
    mut requests: watch::Receiver<Option<SearchRequest>>,
    results: watch::Sender<Option<Arc<SearchJobOutcome>>>,
    shutdown: CancellationToken,
    runner: R,
) -> Result<()> {
    let mut pending = None;

    loop {
        if pending.is_some() && requests.has_changed()? {
            pending = requests.borrow_and_update().clone();
        }
        let request = match pending.take() {
            Some(request) => request,
            None => tokio::select! {
                _ = shutdown.cancelled() => return Ok(()),
                changed = requests.changed() => {
                    changed.context("request search input channel closed")?;
                    results.send_replace(None);
                    let latest = requests.borrow_and_update().clone();
                    let Some(request) = latest else { continue };
                    request
                }
            },
        };

        let key = request.key;
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let worker_runner = runner.clone();
        let mut worker = tokio::task::spawn_blocking(move || {
            worker_runner.run(&request, worker_cancelled.as_ref())
        });

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    cancelled.store(true, Ordering::Relaxed);
                    let _ = worker.await;
                    return Ok(());
                }
                changed = requests.changed() => {
                    changed.context("request search input channel closed")?;
                    cancelled.store(true, Ordering::Relaxed);
                    results.send_replace(None);
                    pending = requests.borrow_and_update().clone();
                }
                joined = &mut worker => {
                    let outcome = joined.unwrap_or_else(|error| SearchJobOutcome::Failed {
                        key,
                        message: Arc::from(format!("search worker failed: {error}")),
                    });
                    results.send_replace(Some(Arc::new(outcome)));
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::RequestTreeModel,
        request_search::{SearchRequestKey, SearchResults},
    };
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    fn request(generation: u64) -> SearchRequest {
        SearchRequest {
            key: SearchRequestKey {
                generation,
                tree_revision: 0,
            },
            query: Arc::from("query"),
            tree: Arc::new(RequestTreeModel::default()),
        }
    }

    #[tokio::test]
    async fn service_delivers_latest_request_and_shuts_down_cleanly() {
        let shutdown = CancellationToken::new();
        let mut service = start_request_search_service(shutdown.clone());
        for generation in 1..=20 {
            service.client.submit(request(generation));
        }

        let latest = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                service
                    .results
                    .changed()
                    .await
                    .expect("result channel should remain open");
                let outcome = service.results.borrow_and_update().clone();
                if let Some(outcome) = outcome
                    && outcome.key().generation == 20
                {
                    break outcome;
                }
            }
        })
        .await
        .expect("latest search should complete");
        assert!(matches!(&*latest, SearchJobOutcome::Completed { .. }));

        shutdown.cancel();
        service
            .task
            .await
            .expect("service task should join")
            .expect("service should shut down successfully");
    }

    #[derive(Clone)]
    struct ControlledRunner {
        started: tokio::sync::mpsc::UnboundedSender<u64>,
        cancelled: tokio::sync::mpsc::UnboundedSender<u64>,
        active: Arc<AtomicUsize>,
        max_active: Arc<AtomicUsize>,
    }

    impl SearchRunner for ControlledRunner {
        fn run(&self, request: &SearchRequest, cancelled: &AtomicBool) -> SearchJobOutcome {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active.fetch_max(active, Ordering::SeqCst);
            self.started
                .send(request.key.generation)
                .expect("test should receive worker start");

            if request.key.generation == 1 {
                while !cancelled.load(Ordering::Relaxed) {
                    std::thread::yield_now();
                }
                self.cancelled
                    .send(request.key.generation)
                    .expect("test should receive worker cancellation");
            }

            self.active.fetch_sub(1, Ordering::SeqCst);
            if cancelled.load(Ordering::Relaxed) {
                SearchJobOutcome::Cancelled { key: request.key }
            } else {
                SearchJobOutcome::Completed {
                    key: request.key,
                    results: Arc::new(SearchResults::new(Vec::new())),
                }
            }
        }
    }

    #[tokio::test]
    async fn scheduler_cancels_active_work_and_runs_only_the_latest_pending_request() {
        let shutdown = CancellationToken::new();
        let (request_tx, request_rx) = watch::channel(None);
        let (result_tx, mut result_rx) = watch::channel(None);
        let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
        let (cancelled_tx, mut cancelled_rx) = tokio::sync::mpsc::unbounded_channel();
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let runner = ControlledRunner {
            started: started_tx,
            cancelled: cancelled_tx,
            active: Arc::clone(&active),
            max_active: Arc::clone(&max_active),
        };
        let task = tokio::spawn(run_service(request_rx, result_tx, shutdown.clone(), runner));
        let client = RequestSearchClient {
            requests: request_tx,
        };

        client.submit(request(1));
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), started_rx.recv())
                .await
                .expect("first worker should start"),
            Some(1)
        );
        client.submit(request(2));
        client.submit(request(3));

        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), cancelled_rx.recv())
                .await
                .expect("first worker should be cancelled"),
            Some(1)
        );
        let latest = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                result_rx
                    .changed()
                    .await
                    .expect("result channel should remain open");
                let outcome = result_rx.borrow_and_update().clone();
                if let Some(outcome) = outcome
                    && outcome.key().generation == 3
                {
                    break outcome;
                }
            }
        })
        .await
        .expect("latest result should arrive");

        assert!(matches!(&*latest, SearchJobOutcome::Completed { .. }));
        assert_eq!(started_rx.try_recv(), Ok(3));
        assert_eq!(
            started_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        );
        assert_eq!(max_active.load(Ordering::SeqCst), 1);
        assert_eq!(active.load(Ordering::SeqCst), 0);

        shutdown.cancel();
        task.await
            .expect("service task should join")
            .expect("service should shut down successfully");
    }

    #[tokio::test]
    async fn cancelling_an_idle_service_releases_its_last_result() {
        let shutdown = CancellationToken::new();
        let mut service = start_request_search_service(shutdown.clone());
        service.client.submit(request(1));
        let outcome = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                service
                    .results
                    .changed()
                    .await
                    .expect("result channel should remain open");
                if let Some(outcome) = service.results.borrow_and_update().clone() {
                    break outcome;
                }
            }
        })
        .await
        .expect("result should arrive");
        assert!(matches!(&*outcome, SearchJobOutcome::Completed { .. }));

        service.client.cancel();
        tokio::time::timeout(Duration::from_secs(2), service.results.changed())
            .await
            .expect("cleared result should arrive")
            .expect("result channel should remain open");
        assert!(service.results.borrow_and_update().is_none());

        shutdown.cancel();
        service
            .task
            .await
            .expect("service task should join")
            .expect("service should shut down successfully");
    }
}

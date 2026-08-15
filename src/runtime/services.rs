use anyhow::{Context, Result, anyhow};
use tokio::{task::JoinHandle, task::JoinSet, time};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ServiceKind {
    Proxy,
    CertificateDownload,
    Logger,
    BodyPumps,
    Decoder,
    RequestSearch,
}

#[derive(Debug)]
pub(super) struct ServiceCompletion {
    pub kind: ServiceKind,
    pub result: Result<()>,
}

pub(super) struct ServiceSupervisor {
    tasks: JoinSet<ServiceCompletion>,
    shutdown: CancellationToken,
}

impl ServiceSupervisor {
    pub fn new(shutdown: CancellationToken) -> Self {
        Self {
            tasks: JoinSet::new(),
            shutdown,
        }
    }

    pub fn track_result(&mut self, kind: ServiceKind, task: JoinHandle<Result<()>>) {
        self.tasks.spawn(async move {
            let result = task
                .await
                .with_context(|| format!("{kind:?} service task failed to join"))
                .and_then(|result| result);
            ServiceCompletion { kind, result }
        });
    }

    pub fn track_infallible(&mut self, kind: ServiceKind, task: JoinHandle<()>) {
        self.tasks.spawn(async move {
            let result = task
                .await
                .with_context(|| format!("{kind:?} service task failed to join"));
            ServiceCompletion { kind, result }
        });
    }

    pub async fn join_next(&mut self) -> Option<Result<ServiceCompletion>> {
        self.tasks.join_next().await.map(|result| {
            result.map_err(|error| anyhow!(error).context("service supervisor task failed"))
        })
    }

    pub async fn shutdown(&mut self, grace: std::time::Duration) {
        self.shutdown.cancel();
        if time::timeout(grace, async {
            while self.tasks.join_next().await.is_some() {}
        })
        .await
        .is_err()
        {
            self.tasks.abort_all();
            while self.tasks.join_next().await.is_some() {}
        }
    }
}

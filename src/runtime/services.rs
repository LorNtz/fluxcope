use anyhow::{Context, Result, anyhow};
use tokio::{task::JoinHandle, task::JoinSet, time};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ServiceKind {
    Proxy,
    ControlRpc,
    CertificateDownload,
    Logger,
    BodyPumps,
    Decoder,
    RequestSearch,
    SettingsTransactions,
}

impl ServiceKind {
    pub(super) const fn is_fatal(self) -> bool {
        !matches!(self, Self::CertificateDownload | Self::Logger)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn control_rpc_service_is_tracked_by_the_shared_supervisor() {
        let shutdown = CancellationToken::new();
        let mut supervisor = ServiceSupervisor::new(shutdown.clone());
        let task = tokio::spawn(async move {
            shutdown.cancelled().await;
            Ok(())
        });
        supervisor.track_result(ServiceKind::ControlRpc, task);

        supervisor.shutdown(std::time::Duration::from_secs(1)).await;

        assert!(supervisor.join_next().await.is_none());
    }

    #[tokio::test]
    async fn unexpected_control_rpc_exit_preserves_its_fatal_error() {
        let shutdown = CancellationToken::new();
        let mut supervisor = ServiceSupervisor::new(shutdown);
        supervisor.track_result(
            ServiceKind::ControlRpc,
            tokio::spawn(async { Err(anyhow!("control listener failed")) }),
        );

        let completion = supervisor
            .join_next()
            .await
            .expect("tracked completion")
            .expect("supervisor join");

        assert_eq!(completion.kind, ServiceKind::ControlRpc);
        let error = completion.result.expect_err("fatal control service error");
        assert!(error.to_string().contains("control listener failed"));
    }
}

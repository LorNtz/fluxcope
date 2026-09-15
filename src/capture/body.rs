use std::{future::Future, time::Duration};

use anyhow::{Result, anyhow};
use http_body_util::{
    BodyExt,
    channel::{Channel, Sender},
};
use hudsucker::Body;
use hyper::body::Bytes;
use tokio::time;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use super::{BodySide, CaptureHandle};

#[derive(Clone)]
pub(crate) struct BodyTaskTracker {
    tasks: TaskTracker,
    shutdown: CancellationToken,
}

impl BodyTaskTracker {
    pub fn new(shutdown: CancellationToken) -> Self {
        Self {
            tasks: TaskTracker::new(),
            shutdown,
        }
    }

    pub fn spawn<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.tasks.spawn(future);
    }

    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown.clone()
    }

    pub async fn wait_for_shutdown(self, grace: Duration) -> Result<()> {
        self.shutdown.cancelled().await;
        self.tasks.close();
        time::timeout(grace, self.tasks.wait())
            .await
            .map_err(|_| anyhow!("body pumps did not stop within {grace:?}"))
    }
}

pub(crate) type BodySender = Sender<Bytes, hudsucker::Error>;

/// Buffer one frame, preserving backpressure and separate abnormal termination.
pub(crate) fn body_channel() -> (BodySender, Body) {
    let (sender, body) = Channel::new(1);
    (sender, Body::from(body.boxed()))
}

pub(crate) fn tee_body(
    mut source: Body,
    capture: CaptureHandle,
    side: BodySide,
    tasks: &BodyTaskTracker,
) -> Body {
    let (mut sender, destination) = body_channel();
    let shutdown = tasks.shutdown_token();
    tasks.spawn(async move {
        // The channel has no receiver-closed future. A downstream drop is
        // observed on send; runtime shutdown cancels a stalled source. Capture
        // admission bounds the total number of these pumps.
        loop {
            let next = tokio::select! {
                _ = shutdown.cancelled() => {
                    capture.cancel(side);
                    sender.abort(std::io::Error::new(std::io::ErrorKind::Interrupted, "proxy shutdown").into());
                    return;
                }
                next = source.frame() => next,
            };
            match next {
                Some(Ok(frame)) => {
                    let candidate = frame.data_ref().cloned();
                    let sent = tokio::select! {
                        _ = shutdown.cancelled() => {
                            capture.cancel(side);
                            sender.abort(std::io::Error::new(std::io::ErrorKind::Interrupted, "proxy shutdown").into());
                            return;
                        }
                        sent = sender.send(frame) => sent,
                    };
                    if sent.is_err() {
                        capture.cancel(side);
                        return;
                    }
                    if let Some(candidate) = candidate {
                        capture.append(side, &candidate);
                    }
                }
                Some(Err(error)) => {
                    capture.fail(side, format!("source body failed: {error}"));
                    sender.abort(error);
                    return;
                }
                None => {
                    capture.complete(side);
                    return;
                }
            }
        }
    });
    destination
}

pub(crate) fn drain_body(mut source: Body, capture: CaptureHandle, tasks: &BodyTaskTracker) {
    let shutdown = tasks.shutdown_token();
    tasks.spawn(async move {
        loop {
            let next = tokio::select! {
                _ = shutdown.cancelled() => {
                    capture.cancel(BodySide::Request);
                    return;
                }
                next = source.frame() => next,
            };
            match next {
                Some(Ok(frame)) => {
                    if let Some(chunk) = frame.data_ref() {
                        capture.append(BodySide::Request, chunk);
                    }
                }
                Some(Err(error)) => {
                    capture.fail(BodySide::Request, format!("request drain failed: {error}"));
                    return;
                }
                None => {
                    capture.complete(BodySide::Request);
                    return;
                }
            }
        }
    });
}

#[cfg(test)]
pub(crate) async fn body_bytes(body: Body) -> Result<Bytes, hudsucker::Error> {
    Ok(body.collect().await?.to_bytes())
}

#[cfg(test)]
mod tests;

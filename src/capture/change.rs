use std::{collections::VecDeque, sync::Arc};

use parking_lot::Mutex;
use tokio::sync::watch;

use super::CaptureSequence;

const RETAINED_CHANGES: usize = 4_096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureChangeKind {
    Admitted,
    RecordUpdated,
    RetentionEviction,
    ExplicitDelete,
    Clear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureChange {
    pub epoch: u64,
    pub sequence: CaptureSequence,
    pub revision: u64,
    pub kind: CaptureChangeKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureChangeError {
    Gap {
        expected_epoch: u64,
        oldest_available_epoch: u64,
    },
}

#[derive(Clone)]
pub struct CaptureChangeFeed {
    inner: Arc<CaptureChangeFeedInner>,
}

struct CaptureChangeFeedInner {
    state: Mutex<ChangeState>,
    epoch_tx: watch::Sender<u64>,
}

struct ChangeState {
    epoch: u64,
    retained: VecDeque<CaptureChange>,
}

impl Default for CaptureChangeFeed {
    fn default() -> Self {
        Self::new()
    }
}

impl CaptureChangeFeed {
    pub fn new() -> Self {
        let (epoch_tx, _) = watch::channel(0);
        Self {
            inner: Arc::new(CaptureChangeFeedInner {
                state: Mutex::new(ChangeState {
                    epoch: 0,
                    retained: VecDeque::with_capacity(RETAINED_CHANGES),
                }),
                epoch_tx,
            }),
        }
    }

    pub fn epoch(&self) -> u64 {
        self.inner.state.lock().epoch
    }

    pub fn subscribe(&self) -> CaptureChangeSubscription {
        let next_epoch = self
            .epoch()
            .checked_add(1)
            .expect("capture change epoch exhausted");
        CaptureChangeSubscription {
            feed: self.clone(),
            epoch_rx: self.inner.epoch_tx.subscribe(),
            next_epoch,
        }
    }

    pub(crate) fn publish(
        &self,
        sequence: CaptureSequence,
        revision: u64,
        kind: CaptureChangeKind,
    ) -> CaptureChange {
        let change = {
            let mut state = self.inner.state.lock();
            state.epoch = state
                .epoch
                .checked_add(1)
                .expect("capture change epoch exhausted");
            let change = CaptureChange {
                epoch: state.epoch,
                sequence,
                revision,
                kind,
            };
            if state.retained.len() == RETAINED_CHANGES {
                state.retained.pop_front();
            }
            state.retained.push_back(change);
            change
        };
        let _ = self.inner.epoch_tx.send_replace(change.epoch);
        change
    }
}

pub struct CaptureChangeSubscription {
    feed: CaptureChangeFeed,
    epoch_rx: watch::Receiver<u64>,
    next_epoch: u64,
}

impl CaptureChangeSubscription {
    pub async fn recv(&mut self) -> Result<CaptureChange, CaptureChangeError> {
        loop {
            if let Some(change) = self.try_read()? {
                return Ok(change);
            }
            let _ = self.epoch_rx.changed().await;
        }
    }

    fn try_read(&mut self) -> Result<Option<CaptureChange>, CaptureChangeError> {
        let state = self.feed.inner.state.lock();
        let Some(oldest) = state.retained.front().copied() else {
            return Ok(None);
        };
        if self.next_epoch < oldest.epoch {
            return Err(CaptureChangeError::Gap {
                expected_epoch: self.next_epoch,
                oldest_available_epoch: oldest.epoch,
            });
        }
        if self.next_epoch > state.epoch {
            return Ok(None);
        }
        let offset = self.next_epoch.saturating_sub(oldest.epoch) as usize;
        let Some(change) = state.retained.get(offset).copied() else {
            return Ok(None);
        };
        self.next_epoch = change
            .epoch
            .checked_add(1)
            .expect("capture change epoch exhausted");
        Ok(Some(change))
    }
}

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Debug)]
pub struct RecordingState {
    enabled: Arc<AtomicBool>,
}

impl RecordingState {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled: Arc::new(AtomicBool::new(enabled)),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn toggle(&self) -> bool {
        self.enabled.fetch_xor(true, Ordering::Relaxed) ^ true
    }
}

impl Default for RecordingState {
    fn default() -> Self {
        Self::new(true)
    }
}

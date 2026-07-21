use std::time::Duration;

use crate::logging::LoggingPolicy;

#[derive(Clone, Debug, Default)]
pub(super) struct RuntimePolicy {
    pub logging: LoggingPolicy,
    pub render: RenderPolicy,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct RenderPolicy {
    pub frame_interval: Duration,
    pub max_events_per_turn: usize,
    pub event_budget: Duration,
    pub metrics_interval: Duration,
    pub shutdown_grace: Duration,
}

impl Default for RenderPolicy {
    fn default() -> Self {
        Self {
            frame_interval: Duration::from_millis(1_000 / 60),
            max_events_per_turn: 256,
            event_budget: Duration::from_millis(2),
            metrics_interval: Duration::from_secs(1),
            shutdown_grace: Duration::from_secs(3),
        }
    }
}

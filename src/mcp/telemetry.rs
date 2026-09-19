use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

#[derive(Debug, Default)]
pub(crate) struct BrokerTelemetry {
    active_calls: AtomicU64,
    active_probes: AtomicU64,
    saturated_calls: AtomicU64,
    cancelled_calls: AtomicU64,
    connection_failures: AtomicU64,
    bytes_relayed: AtomicU64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct TelemetrySnapshot {
    pub(crate) active_calls: u64,
    pub(crate) active_probes: u64,
    pub(crate) saturated_calls: u64,
    pub(crate) cancelled_calls: u64,
    pub(crate) connection_failures: u64,
    pub(crate) bytes_relayed: u64,
}

#[derive(Debug)]
pub(crate) struct ActivityGuard {
    telemetry: Arc<BrokerTelemetry>,
    kind: ActivityKind,
}

#[derive(Clone, Copy, Debug)]
enum ActivityKind {
    PublicCall,
    LivenessProbe,
}

impl BrokerTelemetry {
    pub(crate) fn begin_public_call(self: Arc<Self>) -> ActivityGuard {
        saturating_increment(&self.active_calls, 1);
        ActivityGuard {
            telemetry: self,
            kind: ActivityKind::PublicCall,
        }
    }

    pub(crate) fn begin_liveness_probe(self: Arc<Self>) -> ActivityGuard {
        saturating_increment(&self.active_probes, 1);
        ActivityGuard {
            telemetry: self,
            kind: ActivityKind::LivenessProbe,
        }
    }

    pub(crate) fn record_saturated_call(&self) {
        saturating_increment(&self.saturated_calls, 1);
    }

    pub(crate) fn record_cancelled_call(&self) {
        saturating_increment(&self.cancelled_calls, 1);
    }

    pub(crate) fn record_connection_failure(&self) {
        saturating_increment(&self.connection_failures, 1);
    }

    pub(crate) fn record_bytes_relayed(&self, bytes: u64) {
        saturating_increment(&self.bytes_relayed, bytes);
    }

    pub(crate) fn snapshot(&self) -> TelemetrySnapshot {
        TelemetrySnapshot {
            active_calls: self.active_calls.load(Ordering::Relaxed),
            active_probes: self.active_probes.load(Ordering::Relaxed),
            saturated_calls: self.saturated_calls.load(Ordering::Relaxed),
            cancelled_calls: self.cancelled_calls.load(Ordering::Relaxed),
            connection_failures: self.connection_failures.load(Ordering::Relaxed),
            bytes_relayed: self.bytes_relayed.load(Ordering::Relaxed),
        }
    }
}

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        let counter = match self.kind {
            ActivityKind::PublicCall => &self.telemetry.active_calls,
            ActivityKind::LivenessProbe => &self.telemetry.active_probes,
        };
        let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            Some(value.saturating_sub(1))
        });
    }
}

fn saturating_increment(counter: &AtomicU64, amount: u64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        Some(value.saturating_add(amount))
    });
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::BrokerTelemetry;

    #[test]
    fn telemetry_guards_track_shared_in_flight_calls_and_probes() {
        let telemetry = Arc::new(BrokerTelemetry::default());
        let call = Arc::clone(&telemetry).begin_public_call();
        let probe_a = Arc::clone(&telemetry).begin_liveness_probe();
        let probe_b = Arc::clone(&telemetry).begin_liveness_probe();

        let snapshot = telemetry.snapshot();
        assert_eq!(snapshot.active_calls, 1);
        assert_eq!(snapshot.active_probes, 2);

        drop(probe_a);
        drop(call);
        let snapshot = telemetry.snapshot();
        assert_eq!(snapshot.active_calls, 0);
        assert_eq!(snapshot.active_probes, 1);

        drop(probe_b);
        assert_eq!(telemetry.snapshot().active_probes, 0);
    }

    #[test]
    fn telemetry_reports_only_bounded_transport_and_admission_counters() {
        let telemetry = BrokerTelemetry::default();
        telemetry.record_saturated_call();
        telemetry.record_cancelled_call();
        telemetry.record_connection_failure();
        telemetry.record_bytes_relayed(1_024);
        telemetry.record_bytes_relayed(2_048);

        let snapshot = telemetry.snapshot();
        assert_eq!(snapshot.saturated_calls, 1);
        assert_eq!(snapshot.cancelled_calls, 1);
        assert_eq!(snapshot.connection_failures, 1);
        assert_eq!(snapshot.bytes_relayed, 3_072);
    }

    #[test]
    fn byte_counter_saturates_instead_of_wrapping() {
        let telemetry = BrokerTelemetry::default();
        telemetry.record_bytes_relayed(u64::MAX);
        telemetry.record_bytes_relayed(1);

        assert_eq!(telemetry.snapshot().bytes_relayed, u64::MAX);
    }
}

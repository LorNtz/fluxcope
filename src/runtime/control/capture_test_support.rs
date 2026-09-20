#![cfg(all(test, unix))]

use crate::{
    app::{App, SettingsUiContext},
    capture::{CaptureRecord, CaptureRetentionPolicy, CaptureSequence, CapturedExchange},
    control_rpc::{protocol::DeclaredClient, server::ControlCallContext},
    instance::InstanceIdentity,
    logging::LogRetentionPolicy,
    recording::RecordingState,
    settings::{AppSettings, SettingsSession},
};
use hyper::Method;
use std::{
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

pub(super) fn completed_capture(sequence: u64) -> Arc<CaptureRecord> {
    CaptureRecord::from_completed(CapturedExchange {
        sequence: CaptureSequence::new(sequence),
        method: Method::GET,
        uri: format!("https://example.test/{sequence}"),
        mapped_uri: None,
        local_path: None,
        status: Some(200),
        req_headers: vec![("X-Sequence".to_owned(), sequence.to_string())],
        res_headers: vec![],
        req_body: None,
        res_body: None,
    })
}

pub(super) fn runtime_fixture(max_records: usize) -> (InstanceIdentity, App, SettingsSession) {
    let mut launch = AppSettings::default();
    launch.recording.start_record_on_launch = false;
    let settings = SettingsSession::temporary(launch.clone());
    let app = App::with_runtime_policies(
        launch,
        RecordingState::new(false),
        LogRetentionPolicy::default(),
        CaptureRetentionPolicy {
            max_records,
            max_bytes: usize::MAX,
        },
        SettingsUiContext::default(),
    );
    let identity =
        InstanceIdentity::new("127.0.0.1:19028".parse().expect("endpoint")).expect("identity");
    (identity, app, settings)
}

pub(super) fn control_context(request_id: impl Into<String>) -> ControlCallContext {
    ControlCallContext {
        request_id: request_id.into(),
        declared_client: DeclaredClient {
            name: "task-8-test".to_owned(),
            version: "1".to_owned(),
        },
        deadline: Instant::now() + Duration::from_secs(30),
    }
}

#[derive(Default)]
pub(super) struct BlockingGate {
    released: Mutex<bool>,
    changed: Condvar,
}

impl BlockingGate {
    pub(super) fn wait(&self) {
        let mut released = self.released.lock().expect("gate");
        while !*released {
            released = self.changed.wait(released).expect("gate wait");
        }
    }

    pub(super) fn release(&self) {
        *self.released.lock().expect("gate") = true;
        self.changed.notify_all();
    }
}

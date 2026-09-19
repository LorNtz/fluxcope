pub(crate) mod mapping;

use std::sync::Arc;

use tokio::sync::OwnedSemaphorePermit;

use crate::{
    request_policy::RequestPolicy,
    runtime::settings::{
        SettingsRevision, SettingsTransactionOrigin, SettingsTransactionOutcome,
        SettingsTransactionToken,
    },
    settings::{
        AppSettings, ConfigMode, PersistenceMode,
        mapping_ops::{MappingExplanation, MappingObjectRef, MappingValidationResult},
    },
};
#[derive(Clone, Debug)]
pub(crate) struct MappingSettingsSnapshot {
    pub(crate) settings: Arc<AppSettings>,
    pub(crate) revision: SettingsRevision,
    pub(crate) config_mode: ConfigMode,
    pub(crate) persistence: PersistenceMode,
}
#[derive(Clone, Debug)]
pub(crate) struct BeginSettingsTransactionReply {
    pub(crate) settings: Arc<AppSettings>,
    pub(crate) revision: SettingsRevision,
    pub(crate) token: SettingsTransactionToken,
    pub(crate) config_mode: ConfigMode,
    pub(crate) persistence: PersistenceMode,
}
#[derive(Debug)]
pub(crate) struct FinalizedSettingsTransaction {
    pub(crate) settings: Arc<AppSettings>,
    pub(crate) policy: Option<RequestPolicy>,
    pub(crate) origin: SettingsTransactionOrigin,
    pub(crate) outcome: SettingsTransactionOutcome,
    pub(crate) affected: MappingObjectRef,
    pub(crate) persistence: PersistenceMode,
}

impl PartialEq for FinalizedSettingsTransaction {
    fn eq(&self, other: &Self) -> bool {
        self.settings == other.settings
            && self.origin == other.origin
            && self.outcome == other.outcome
            && self.affected == other.affected
            && self.persistence == other.persistence
    }
}

#[derive(Clone, Debug)]
pub(crate) struct MappingSettingsResult {
    pub(crate) revision: SettingsRevision,
    pub(crate) config_mode: ConfigMode,
    pub(crate) persistence: PersistenceMode,
    pub(crate) mapping: mapping::MappingSettingsView,
    pub(crate) worker_permit: Arc<OwnedSemaphorePermit>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MappingValidationReply {
    pub(crate) revision: SettingsRevision,
    pub(crate) config_mode: ConfigMode,
    pub(crate) persistence: PersistenceMode,
    pub(crate) validation: MappingValidationResult,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MappingExplanationReply {
    pub(crate) revision: SettingsRevision,
    pub(crate) config_mode: ConfigMode,
    pub(crate) persistence: PersistenceMode,
    pub(crate) explanation: MappingExplanation,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MappingPreviewReply {
    pub(crate) revision: SettingsRevision,
    pub(crate) config_mode: ConfigMode,
    pub(crate) persistence: PersistenceMode,
    pub(crate) preview: mapping::MappingMutationPreview,
}
